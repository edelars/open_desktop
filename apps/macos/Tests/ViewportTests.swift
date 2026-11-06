import Foundation
import XCTest
@testable import ODShared

// Unit test of Shared/Viewport.swift (S10-11, cursor placement S10-14). XCTest case of the OpenDesktopMacTests target (S11-05a);
// run with `swift test --filter ViewportXCTests` from apps/. It used to be an executable with `@main`; the checks are unchanged.

private var failures = 0

private func check(_ condition: Bool, _ message: @autoclosure () -> String) {
    if !condition {
        failures += 1
        print("FAIL: \(message())")
    }
}

struct ViewportTests {
    static func fitCases() {
        let cases: [(fw: Int, fh: Int, tw: Int, th: Int, expected: Viewport)] = [
            (64, 48, 64, 48, Viewport(x: 0, y: 0, width: 64, height: 48)), // 1:1
            (64, 48, 128, 96, Viewport(x: 0, y: 0, width: 128, height: 96)), // exact 2x
            (64, 48, 100, 100, Viewport(x: 0, y: 12, width: 100, height: 75)), // letterbox
            (64, 48, 200, 50, Viewport(x: 66, y: 0, width: 67, height: 50)), // pillarbox (66.67 -> 67)
            (64, 48, 50, 200, Viewport(x: 0, y: 81, width: 50, height: 38)), // 37.5 -> 38
            (1920, 1080, 2560, 1600, Viewport(x: 0, y: 80, width: 2560, height: 1440)),
            (64, 48, 99, 99, Viewport(x: 0, y: 12, width: 99, height: 74)), // odd margin 25: 12 above, 13 below
            (64, 48, 1, 1, Viewport(x: 0, y: 0, width: 1, height: 1)),
            (64, 48, 0, 10, Viewport(x: 0, y: 0, width: 0, height: 0)), // empty target
        ]
        for c in cases {
            let got = ViewportLayout.viewport(
                mode: .fit, frameWidth: c.fw, frameHeight: c.fh, targetWidth: c.tw, targetHeight: c.th)
            check(got == c.expected, "fit \(c.fw)x\(c.fh) -> \(c.tw)x\(c.th): got \(got), expected \(c.expected)")
        }
        // Properties: inside the target, margins differ by at most one pixel (centred), aspect error below one pixel.
        for (fw, fh) in [(64, 48), (1920, 1080), (640, 480), (3, 7)] {
            for tw in stride(from: 1, through: 400, by: 7) {
                for th in stride(from: 1, through: 300, by: 11) {
                    let v = ViewportLayout.viewport(
                        mode: .fit, frameWidth: fw, frameHeight: fh, targetWidth: tw, targetHeight: th)
                    check(v.width >= 1 && v.height >= 1 && v.width <= tw && v.height <= th, "fit bounds \(fw)x\(fh)->\(tw)x\(th): \(v)")
                    check(v.x >= 0 && v.y >= 0 && v.x + v.width <= tw && v.y + v.height <= th, "fit inside \(fw)x\(fh)->\(tw)x\(th): \(v)")
                    check(abs(v.x - (tw - v.x - v.width)) <= 1 && abs(v.y - (th - v.y - v.height)) <= 1, "fit centred \(v)")
                    check(v.width == tw || v.height == th, "fit touches two sides \(fw)x\(fh)->\(tw)x\(th): \(v)")
                    // Only one side is rounded (half up), by at most half a pixel, so |w*fh - h*fw| <= max(fw, fh) / 2.
                    // A side clamped up to 1 pixel (ideal size below 0.5) is the one case where the rounding rule does not apply.
                    if v.width > 1 && v.height > 1 {
                        let aspectError = abs(v.width * fh - v.height * fw)
                        check(2 * aspectError <= max(fw, fh), "aspect \(fw)x\(fh)->\(tw)x\(th): \(v)")
                    }
                }
            }
        }
    }

    static func pixelExactCases() {
        // (target w, h, expected full viewport, expected visible part, expected uv rect u0 v0 du dv)
        let cases: [(tw: Int, th: Int, full: Viewport, visible: Viewport, uv: (Double, Double, Double, Double))] = [
            (100, 100, Viewport(x: 18, y: 26, width: 64, height: 48), Viewport(x: 18, y: 26, width: 64, height: 48), (0, 0, 1, 1)),
            (64, 48, Viewport(x: 0, y: 0, width: 64, height: 48), Viewport(x: 0, y: 0, width: 64, height: 48), (0, 0, 1, 1)),
            (101, 101, Viewport(x: 18, y: 26, width: 64, height: 48), Viewport(x: 18, y: 26, width: 64, height: 48), (0, 0, 1, 1)), // odd margins 18 / 19, 26 / 27
            (10, 10, Viewport(x: -27, y: -19, width: 64, height: 48), Viewport(x: 0, y: 0, width: 10, height: 10), (27.0 / 64, 19.0 / 48, 10.0 / 64, 10.0 / 48)),
            (63, 47, Viewport(x: -1, y: -1, width: 64, height: 48), Viewport(x: 0, y: 0, width: 63, height: 47), (1.0 / 64, 1.0 / 48, 63.0 / 64, 47.0 / 48)), // odd crop: floor division cuts the extra pixel on the left/top (column 0 and row 0)
            (200, 30, Viewport(x: 68, y: -9, width: 64, height: 48), Viewport(x: 68, y: 0, width: 64, height: 30), (0, 9.0 / 48, 1, 30.0 / 48)), // crop on one axis only
            (40, 100, Viewport(x: -12, y: 26, width: 64, height: 48), Viewport(x: 0, y: 26, width: 40, height: 48), (12.0 / 64, 0, 40.0 / 64, 1)),
        ]
        for c in cases {
            let full = ViewportLayout.viewport(
                mode: .pixelExact, frameWidth: 64, frameHeight: 48, targetWidth: c.tw, targetHeight: c.th)
            check(full == c.full, "pixelExact \(c.tw)x\(c.th): full \(full), expected \(c.full)")
            let visible = full.clipped(targetWidth: c.tw, targetHeight: c.th)
            check(visible == c.visible, "pixelExact \(c.tw)x\(c.th): visible \(visible), expected \(c.visible)")
            check(visible.x >= 0 && visible.y >= 0 && visible.x + visible.width <= c.tw && visible.y + visible.height <= c.th,
                  "pixelExact \(c.tw)x\(c.th): visible \(visible) leaves the target")
            let uv = ViewportLayout.visibleUV(full: full, visible: visible)
            check(abs(uv.u0 - c.uv.0) < 1e-12 && abs(uv.v0 - c.uv.1) < 1e-12 && abs(uv.du - c.uv.2) < 1e-12 && abs(uv.dv - c.uv.3) < 1e-12,
                  "pixelExact \(c.tw)x\(c.th): uv \(uv), expected \(c.uv)")
            // 1:1 even when cropped: the visible part is exactly the texel columns/rows u0*64 ..< (u0+du)*64.
            check(abs(uv.du * 64 - Double(visible.width)) < 1e-9 && abs(uv.dv * 48 - Double(visible.height)) < 1e-9, "pixelExact \(c.tw)x\(c.th): not 1:1")
            // A cropped axis keeps the frame centre (within half a pixel): the visible part is the middle of the frame.
            if visible.width < 64 {
                check(abs((uv.u0 + uv.du / 2) * 64 - 32) <= 0.5, "pixelExact \(c.tw)x\(c.th): x crop off centre")
            }
            if visible.height < 48 {
                check(abs((uv.v0 + uv.dv / 2) * 48 - 24) <= 0.5, "pixelExact \(c.tw)x\(c.th): y crop off centre")
            }
        }
        // A viewport that does not overlap the target is empty.
        check(Viewport(x: 100, y: 0, width: 10, height: 10).clipped(targetWidth: 50, targetHeight: 50).isEmpty, "no overlap -> empty")
        // Fit viewports always lie inside the target (clipping is the identity).
        let fit = ViewportLayout.viewport(mode: .fit, frameWidth: 64, frameHeight: 48, targetWidth: 100, targetHeight: 100)
        check(fit.clipped(targetWidth: 100, targetHeight: 100) == fit, "fit is not clipped")
    }

    static func drawableSizeCases() {
        let retina = ViewportLayout.drawableSize(logicalWidth: 640, logicalHeight: 480, scale: 2)
        check(retina.width == 1280 && retina.height == 960, "retina drawable \(retina)")
        let fractional = ViewportLayout.drawableSize(logicalWidth: 101, logicalHeight: 75, scale: 1.5)
        check(fractional.width == 152 && fractional.height == 113, "fractional scale drawable \(fractional)") // 151.5 -> 152, 112.5 -> 113
        let tiny = ViewportLayout.drawableSize(logicalWidth: 0, logicalHeight: 0, scale: 2)
        check(tiny.width == 1 && tiny.height == 1, "zero logical size is clamped to 1x1")
    }

    static func inverseMapping() {
        // Round trip: the centre of every drawable pixel inside the viewport maps to a guest pixel inside the frame,
        // and for an integer scale the guest pixel is exactly the one that was drawn there.
        let (fw, fh) = (64, 48)
        for (tw, th) in [(64, 48), (128, 96), (100, 100), (200, 50), (50, 200), (97, 61)] {
            let v = ViewportLayout.viewport(mode: .fit, frameWidth: fw, frameHeight: fh, targetWidth: tw, targetHeight: th)
            var inside = 0
            for py in 0..<th {
                for px in 0..<tw {
                    let g = ViewportLayout.guestPixel(
                        pixelX: Double(px) + 0.5, pixelY: Double(py) + 0.5, viewport: v, frameWidth: fw, frameHeight: fh)
                    let isInside = px >= v.x && px < v.x + v.width && py >= v.y && py < v.y + v.height
                    check((g != nil) == isInside, "inside/outside \(tw)x\(th) at \(px),\(py)")
                    guard let g else { continue }
                    inside += 1
                    check(g.x >= 0 && g.x < fw && g.y >= 0 && g.y < fh, "guest range \(tw)x\(th) at \(px),\(py): \(g)")
                    // Error against the real-valued ideal (continuous inverse) is at most half a guest pixel (< 1 as S10-11 requires).
                    let idealX = (Double(px - v.x) + 0.5) * Double(fw) / Double(v.width)
                    let idealY = (Double(py - v.y) + 0.5) * Double(fh) / Double(v.height)
                    check(abs(Double(g.x) + 0.5 - idealX) <= 0.5 + 1e-9 && abs(Double(g.y) + 0.5 - idealY) <= 0.5 + 1e-9, "rounding error \(tw)x\(th) at \(px),\(py): \(g) vs \(idealX),\(idealY)")
                }
            }
            check(inside == v.width * v.height, "viewport area \(tw)x\(th): \(inside) vs \(v.width * v.height)")
            // Corners of the viewport map to the corner guest pixels; every guest pixel is hit when upscaling (v.width >= fw).
            let first = ViewportLayout.guestPixel(pixelX: Double(v.x) + 0.5, pixelY: Double(v.y) + 0.5, viewport: v, frameWidth: fw, frameHeight: fh)
            let last = ViewportLayout.guestPixel(pixelX: Double(v.x + v.width) - 0.5, pixelY: Double(v.y + v.height) - 0.5, viewport: v, frameWidth: fw, frameHeight: fh)
            check(first?.x == 0 && first?.y == 0, "first corner \(tw)x\(th): \(String(describing: first))")
            check(last?.x == fw - 1 && last?.y == fh - 1, "last corner \(tw)x\(th): \(String(describing: last))")
            if v.width >= fw && v.height >= fh {
                var hit = Set<Int>()
                var previousX = -1
                for px in v.x..<(v.x + v.width) {
                    let g = ViewportLayout.guestPixel(pixelX: Double(px) + 0.5, pixelY: Double(v.y) + 0.5, viewport: v, frameWidth: fw, frameHeight: fh)
                    if let g { hit.insert(g.x); check(g.x >= previousX, "x not monotonic \(tw)x\(th) at \(px)"); previousX = g.x }
                }
                check(hit.count == fw, "upscale \(tw)x\(th) hits \(hit.count) of \(fw) guest columns")
            }
        }
        // Integer 2x: each guest pixel owns a 2x2 block; the block centre maps back to it.
        let v2 = ViewportLayout.viewport(mode: .fit, frameWidth: fw, frameHeight: fh, targetWidth: 128, targetHeight: 96)
        for gy in 0..<fh {
            for gx in 0..<fw {
                let g = ViewportLayout.guestPixel(
                    pixelX: Double(gx * 2) + 1, pixelY: Double(gy * 2) + 1, viewport: v2, frameWidth: fw, frameHeight: fh)
                check(g?.x == gx && g?.y == gy, "round trip 2x guest \(gx),\(gy): \(String(describing: g))")
                // Both drawable pixels of the block (centres at +0.5 and +1.5) belong to the same guest pixel.
                for (dx, dy) in [(0.5, 0.5), (1.5, 0.5), (0.5, 1.5), (1.5, 1.5)] {
                    let h = ViewportLayout.guestPixel(
                        pixelX: Double(gx * 2) + dx, pixelY: Double(gy * 2) + dy, viewport: v2, frameWidth: fw, frameHeight: fh)
                    check(h?.x == gx && h?.y == gy, "2x block \(gx),\(gy) offset \(dx),\(dy): \(String(describing: h))")
                }
            }
        }
        // Pixel-exact: window pixel minus the viewport origin is the guest pixel.
        let vp = ViewportLayout.viewport(mode: .pixelExact, frameWidth: fw, frameHeight: fh, targetWidth: 100, targetHeight: 100)
        let g = ViewportLayout.guestPixel(pixelX: 18.5, pixelY: 26.5, viewport: vp, frameWidth: fw, frameHeight: fh)
        check(g?.x == 0 && g?.y == 0, "pixelExact origin: \(String(describing: g))")
        let last = ViewportLayout.guestPixel(pixelX: 81.5, pixelY: 73.5, viewport: vp, frameWidth: fw, frameHeight: fh)
        check(last?.x == 63 && last?.y == 47, "pixelExact last: \(String(describing: last))")
        // Exhaustive pixel-exact inverse, including a cropped frame (negative origin) and odd margins.
        for (tw, th) in [(100, 100), (101, 101), (10, 10), (63, 47), (200, 30), (40, 100)] {
            let v = ViewportLayout.viewport(mode: .pixelExact, frameWidth: fw, frameHeight: fh, targetWidth: tw, targetHeight: th)
            for py in 0..<th {
                for px in 0..<tw {
                    let g = ViewportLayout.guestPixel(pixelX: Double(px) + 0.5, pixelY: Double(py) + 0.5, viewport: v, frameWidth: fw, frameHeight: fh)
                    let isInside = px >= v.x && px < v.x + v.width && py >= v.y && py < v.y + v.height
                    check((g != nil) == isInside, "pixelExact inside/outside \(tw)x\(th) at \(px),\(py)")
                    if let g { check(g.x == px - v.x && g.y == py - v.y, "pixelExact exact \(tw)x\(th) at \(px),\(py): \(g)") }
                }
            }
        }
        check(ViewportLayout.guestPixel(pixelX: 17.5, pixelY: 30, viewport: vp, frameWidth: fw, frameHeight: fh) == nil, "margin is outside")
        check(ViewportLayout.guestPixel(pixelX: 5, pixelY: 5, viewport: Viewport(x: 0, y: 0, width: 0, height: 0), frameWidth: fw, frameHeight: fh) == nil, "empty viewport")
    }

    // MARK: S10-14 cursor placement (FRAME_TRANSPORT_V1.md 10.8, check 8)

    /// For frames, targets, both modes, cursor sizes, hotspots and positions: the pixel centres of the drawable inside the
    /// cursor rectangle map (through `guestPixel`) to guest pixels inside the clipped image rectangle, and the narrowed texture
    /// coordinates give the image texel `guest - (pos - hot)` there; the other way round, a guest pixel inside the clipped
    /// rectangle that shows in the visible part lies in the rectangle. The rectangle never leaves the visible part. The
    /// texel-boundary band (centres within 1e-6 of an edge) is skipped, as the spec allows.
    static func cursorOverlay() {
        let frames = [(64, 48), (40, 30), (7, 5)]
        let targets = [(64, 48), (40, 30), (100, 100), (10, 8), (128, 96), (33, 17), (200, 50), (3, 3)]
        let sizes = [(4, 4), (16, 16), (64, 64), (1, 1), (3, 7)]
        for (fw, fh) in frames {
            for mode in [PresentMode.fit, .pixelExact] {
                for (tw, th) in targets {
                    let full = ViewportLayout.viewport(mode: mode, frameWidth: fw, frameHeight: fh, targetWidth: tw, targetHeight: th)
                    let visible = full.clipped(targetWidth: tw, targetHeight: th)
                    let positions = [(0, 0), (fw - 1, fh - 1), (fw / 2, fh / 2), (1, 1), (max(0, fw - 2), 2), (fw + 5, 0), (0, fh + 9)]
                    for (cw, ch) in sizes {
                        for (hx, hy) in [(0, 0), (cw - 1, ch - 1), (cw / 2, ch / 2)] {
                            for (px, py) in positions {
                                checkPlacement(
                                    fw: fw, fh: fh, mode: mode, tw: tw, th: th, full: full, visible: visible,
                                    posX: px, posY: py, hotX: hx, hotY: hy, cw: cw, ch: ch)
                            }
                        }
                    }
                }
            }
        }
    }

    private static func checkPlacement(
        fw: Int, fh: Int, mode: PresentMode, tw: Int, th: Int, full: Viewport, visible: Viewport,
        posX: Int, posY: Int, hotX: Int, hotY: Int, cw: Int, ch: Int
    ) {
        let label = "\(mode) frame \(fw)x\(fh) target \(tw)x\(th) cursor \(cw)x\(ch) hot \(hotX),\(hotY) pos \(posX),\(posY)"
        let gx0 = posX - hotX
        let gy0 = posY - hotY
        let cx0 = max(gx0, 0)
        let cy0 = max(gy0, 0)
        let cx1 = min(gx0 + cw, fw)
        let cy1 = min(gy0 + ch, fh)
        let placement = CursorOverlay.placement(
            frameWidth: fw, frameHeight: fh, posX: posX, posY: posY, hotX: hotX, hotY: hotY, width: cw, height: ch,
            full: full, visible: visible)
        if cx1 <= cx0 || cy1 <= cy0 || visible.isEmpty {
            check(placement == nil, "outside the frame must not draw: \(label)")
            return
        }
        if let p = placement {
            // 10.8 step 4: the viewport of the cursor is inside the render target (the visible part).
            check(p.x0 >= Double(visible.x) && p.y0 >= Double(visible.y)
                && p.x1 <= Double(visible.x + visible.width) && p.y1 <= Double(visible.y + visible.height)
                && p.x1 > p.x0 && p.y1 > p.y0, "rect leaves the visible part: \(p) \(label)")
            check(p.x0 >= 0 && p.y0 >= 0 && p.x1 <= Double(tw) && p.y1 <= Double(th), "rect leaves the render target: \(label)")
            let eps = 1e-9
            check(p.u0 >= -eps && p.v0 >= -eps && p.u0 + p.du <= 1 + eps && p.v0 + p.dv <= 1 + eps && p.du > 0 && p.dv > 0,
                  "texture coordinates leave 0...1: \(p) \(label)")
        }
        // Pixel centres over the part of the drawable the image can touch (bounding box of the unclipped-by-target rectangle).
        let boxX0 = max(visible.x, Int((Double(full.x) + Double(cx0) * Double(full.width) / Double(fw)).rounded(.down)) - 1)
        let boxX1 = min(visible.x + visible.width, Int((Double(full.x) + Double(cx1) * Double(full.width) / Double(fw)).rounded(.up)) + 1)
        let boxY0 = max(visible.y, Int((Double(full.y) + Double(cy0) * Double(full.height) / Double(fh)).rounded(.down)) - 1)
        let boxY1 = min(visible.y + visible.height, Int((Double(full.y) + Double(cy1) * Double(full.height) / Double(fh)).rounded(.up)) + 1)
        if boxX1 <= boxX0 || boxY1 <= boxY0 {
            check(placement == nil, "no drawable pixel can show the image, yet a rectangle was returned: \(label)")
            return
        }
        for y in boxY0..<boxY1 {
            for x in boxX0..<boxX1 {
                let centreX = Double(x) + 0.5
                let centreY = Double(y) + 0.5
                guard let guest = ViewportLayout.guestPixel(
                    pixelX: centreX, pixelY: centreY, viewport: full, frameWidth: fw, frameHeight: fh) else { continue }
                let insideImage = guest.x >= cx0 && guest.x < cx1 && guest.y >= cy0 && guest.y < cy1
                // Skip centres on a guest texel boundary (the GPU may pick the neighbour there, DISPLAY.md 3).
                let fx = (centreX - Double(full.x)) * Double(fw) / Double(full.width)
                let fy = (centreY - Double(full.y)) * Double(fh) / Double(full.height)
                if abs(fx - fx.rounded()) < 1e-6 || abs(fy - fy.rounded()) < 1e-6 { continue }
                let inRect = placement.map { centreX >= $0.x0 && centreX < $0.x1 && centreY >= $0.y0 && centreY < $0.y1 } ?? false
                check(inRect == insideImage, "pixel \(x),\(y): in rectangle \(inRect), guest pixel \(guest) inside image \(insideImage): \(label)")
                if inRect, let p = placement {
                    let u = p.u0 + (centreX - p.x0) / (p.x1 - p.x0) * p.du
                    let v = p.v0 + (centreY - p.y0) / (p.y1 - p.y0) * p.dv
                    let texelX = Int((u * Double(cw)).rounded(.down))
                    let texelY = Int((v * Double(ch)).rounded(.down))
                    check(texelX == guest.x - gx0 && texelY == guest.y - gy0,
                          "pixel \(x),\(y): texel \(texelX),\(texelY), expected \(guest.x - gx0),\(guest.y - gy0): \(label)")
                }
            }
        }
    }

    /// The case the spec requires by name: pixel-exact, drawable smaller than the frame, cursor at the cropped edges.
    static func cursorOverlayRequiredCases() {
        let full = ViewportLayout.viewport(mode: .pixelExact, frameWidth: 64, frameHeight: 48, targetWidth: 40, targetHeight: 30)
        let visible = full.clipped(targetWidth: 40, targetHeight: 30)
        check(full == Viewport(x: -12, y: -9, width: 64, height: 48), "pixel-exact 64x48 in 40x30: \(full)")
        check(visible == Viewport(x: 0, y: 0, width: 40, height: 30), "visible part: \(visible)")
        // Top-left: image 4x4 at guest (10, 8): columns 10, 11 are cropped away (visible guest x starts at 12), rows 8 as well.
        let topLeft = CursorOverlay.placement(
            frameWidth: 64, frameHeight: 48, posX: 10, posY: 8, hotX: 0, hotY: 0, width: 4, height: 4, full: full, visible: visible)
        check(topLeft == CursorPlacement(x0: 0, y0: 0, x1: 2, y1: 3, u0: 0.5, v0: 0.25, du: 0.5, dv: 0.75),
              "top-left cropped: \(String(describing: topLeft))")
        // Bottom-right: image at guest (50, 37): the visible guest x ends at 52, y at 39.
        let bottomRight = CursorOverlay.placement(
            frameWidth: 64, frameHeight: 48, posX: 50, posY: 37, hotX: 0, hotY: 0, width: 4, height: 4, full: full, visible: visible)
        check(bottomRight == CursorPlacement(x0: 38, y0: 28, x1: 40, y1: 30, u0: 0, v0: 0, du: 0.5, dv: 0.5),
              "bottom-right cropped: \(String(describing: bottomRight))")
        // Entirely in the cropped part: inside the frame, outside the drawable.
        let hidden = CursorOverlay.placement(
            frameWidth: 64, frameHeight: 48, posX: 2, posY: 2, hotX: 0, hotY: 0, width: 4, height: 4, full: full, visible: visible)
        check(hidden == nil, "cursor in the cropped part is not drawn: \(String(describing: hidden))")
        // Negative image corner (pos < hot) at the frame corner, 1:1.
        let fullOne = ViewportLayout.viewport(mode: .fit, frameWidth: 64, frameHeight: 48, targetWidth: 64, targetHeight: 48)
        let corner = CursorOverlay.placement(
            frameWidth: 64, frameHeight: 48, posX: 0, posY: 0, hotX: 2, hotY: 2, width: 4, height: 4, full: fullOne, visible: fullOne)
        check(corner == CursorPlacement(x0: 0, y0: 0, x1: 2, y1: 2, u0: 0.5, v0: 0.5, du: 0.5, dv: 0.5), "negative corner: \(String(describing: corner))")
    }
}

/// XCTest wrapper (S11-05a): each former step of `main()` is a test case; the checks above are unchanged and still count failures.
final class ViewportXCTests: XCTestCase {
    override func setUp() {
        failures = 0
    }

    private func finish(_ name: String) {
        XCTAssertEqual(failures, 0, "viewport \(name): \(failures) check(s) failed (see FAIL lines above)")
    }

    func testFitCases() {
        ViewportTests.fitCases()
        finish("fitCases")
    }

    func testPixelExactCases() {
        ViewportTests.pixelExactCases()
        finish("pixelExactCases")
    }

    func testDrawableSizeCases() {
        ViewportTests.drawableSizeCases()
        finish("drawableSizeCases")
    }

    func testInverseMapping() {
        ViewportTests.inverseMapping()
        finish("inverseMapping")
    }

    func testCursorOverlay() {
        ViewportTests.cursorOverlay()
        finish("cursorOverlay")
    }

    func testCursorOverlayRequiredCases() {
        ViewportTests.cursorOverlayRequiredCases()
        finish("cursorOverlayRequiredCases")
    }
}
