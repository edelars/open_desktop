import Foundation

// Unit test of Sources/Viewport.swift (S10-11). Plain executable, exit status 0 on success:
//   xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library \
//     apps/macos/Sources/Viewport.swift apps/macos/Tests/ViewportTests.swift -o target/viewport-tests && target/viewport-tests

private var failures = 0

private func check(_ condition: Bool, _ message: @autoclosure () -> String) {
    if !condition {
        failures += 1
        print("FAIL: \(message())")
    }
}

@main
struct ViewportTests {
    static func main() {
        fitCases()
        pixelExactCases()
        drawableSizeCases()
        inverseMapping()
        if failures != 0 {
            print("viewport tests: \(failures) failure(s)")
            exit(1)
        }
        print("viewport tests ok")
    }

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
}
