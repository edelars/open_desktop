import Foundation

// Translation of window events into input values (INPUT_TRANSPORT_V1.md sections 5.3 to 5.5, task S10-16): the
// AppKit point -> physical pixel -> guest pixel chain on top of `ViewportLayout`, fractional accumulation of relative motion
// and of the wheel. Pure functions and values; no AppKit, so the unit test covers them without a window.

/// The frame the presenter shows right now, as far as input is concerned (5.3 step 6).
struct ShownFrame: Equatable {
    var width: Int
    var height: Int
    /// `mode_change_count` of the shown frame.
    var modeChangeCount: UInt64
}

/// Absolute position of one event (the `abs_*` fields of MOTION).
struct InputAbsolute: Equatable {
    var x: UInt32
    var y: UInt32
    var width: UInt32
    var height: UInt32
    var modeChangeCount: UInt64
}

/// Geometry of one event: current bounds, scale and shown frame (nothing is cached between events, section 5.3).
struct InputGeometry {
    let boundsWidth: Double
    let boundsHeight: Double
    let drawableWidth: Int
    let drawableHeight: Int
    let mode: PresentMode
    /// nil when no frame can be the target of an absolute position (not SHOWING, a stale frame, another size).
    let shown: ShownFrame?
    /// Full viewport (may start below 0 in pixel-exact mode); nil without a shown frame.
    let viewport: Viewport?

    init(boundsWidth: Double, boundsHeight: Double, scale: Double, mode: PresentMode, shown: ShownFrame?) {
        self.boundsWidth = boundsWidth
        self.boundsHeight = boundsHeight
        // Step 2: the same value DisplayWindow.refresh puts into layer.drawableSize.
        let drawable = ViewportLayout.drawableSize(logicalWidth: boundsWidth, logicalHeight: boundsHeight, scale: scale)
        drawableWidth = drawable.width
        drawableHeight = drawable.height
        self.mode = mode
        self.shown = shown
        if let shown {
            // Step 4: the full viewport, no clipping.
            viewport = ViewportLayout.viewport(
                mode: mode, frameWidth: shown.width, frameHeight: shown.height,
                targetWidth: drawable.width, targetHeight: drawable.height)
        } else {
            viewport = nil
        }
    }

    /// Guest pixels per physical pixel on each axis (`k` of section 5.3); 1 when no frame is shown (5.4).
    var kx: Double {
        guard let shown, let viewport, !viewport.isEmpty else { return 1 }
        return Double(shown.width) / Double(viewport.width)
    }

    var ky: Double {
        guard let shown, let viewport, !viewport.isEmpty else { return 1 }
        return Double(shown.height) / Double(viewport.height)
    }

    /// Step 3. `pointX/pointY`: the point in view coordinates, origin bottom-left, logical points. The ratio `D/B` is used,
    /// not `backingScaleFactor`, so the edges of the bounds fall exactly on the edges of the drawable. A point on the far edge
    /// (x == Bw, y == 0) belongs to the last physical pixel; points outside the bounds stay outside.
    func physicalPixel(pointX: Double, pointY: Double) -> (x: Int, y: Int) {
        func axis(_ value: Double, _ bounds: Double, _ drawable: Int) -> Int {
            let pixel = Int((value * Double(drawable) / bounds).rounded(.down))
            return value >= 0 && value <= bounds ? min(pixel, drawable - 1) : pixel
        }
        return (
            axis(pointX, boundsWidth, drawableWidth),
            axis(boundsHeight - pointY, boundsHeight, drawableHeight)
        )
    }

    /// Steps 3 to 6. nil when the absolute position cannot be computed (`ABS_VALID = 0`).
    func absolute(pointX: Double, pointY: Double) -> InputAbsolute? {
        guard boundsWidth > 0, boundsHeight > 0, let shown, let viewport else { return nil }
        // A point outside the window bounds (a drag that left the view) never gives an absolute position, also in pixel-exact
        // mode where the cropped viewport could otherwise contain it.
        guard pointX >= 0, pointX <= boundsWidth, pointY >= 0, pointY <= boundsHeight else { return nil }
        let pixel = physicalPixel(pointX: pointX, pointY: pointY)
        // Step 5: the centre of the physical pixel, the texel the GPU samples.
        guard let guest = ViewportLayout.guestPixel(
            pixelX: Double(pixel.x) + 0.5, pixelY: Double(pixel.y) + 0.5, viewport: viewport,
            frameWidth: shown.width, frameHeight: shown.height) else { return nil }
        return InputAbsolute(
            x: UInt32(guest.x), y: UInt32(guest.y), width: UInt32(shown.width), height: UInt32(shown.height),
            modeChangeCount: shown.modeChangeCount)
    }

    /// Step 1 of 5.4: logical points of `NSEvent.deltaX/deltaY` -> guest counts (physical pixels times `k`), not yet truncated.
    func guestDelta(dxPoints: Double, dyPoints: Double) -> (dx: Double, dy: Double) {
        (
            dxPoints * Double(drawableWidth) / boundsWidth * kx,
            dyPoints * Double(drawableHeight) / boundsHeight * ky
        )
    }
}

/// Whole part with the fraction kept for the next event (truncation toward zero, sections 5.4 and 5.5).
struct FractionalAccumulator {
    private(set) var remainder = 0.0

    mutating func add(_ value: Double) -> Int32 {
        guard value.isFinite else { return 0 }
        let total = remainder + value
        let whole = max(Double(Int32.min), min(Double(Int32.max), total.rounded(.towardZero)))
        remainder = total - whole
        return Int32(whole)
    }

    mutating func reset() { remainder = 0 }
}

/// Remainders of the pointer axes and of the wheel; reset on `FOCUS LOST` and when `k` changes (5.4).
struct InputPointerState {
    /// Section 5.5, open item 4 of section 11: 1/120 wheel units for one point of a precise (trackpad) scroll. NOT measured:
    /// 7.5 = 120 / 16, i.e. about one notch per 16 points, a typical line height. A measurement belongs to a run with a real
    /// guest (there is no delivery yet).
    static let wheelUnitsPerPoint = 7.5

    private var x = FractionalAccumulator()
    private var y = FractionalAccumulator()
    private var wheel = FractionalAccumulator()
    private var pan = FractionalAccumulator()
    private var lastK: (x: Double, y: Double)?

    /// Section 5.4: `dxPoints/dyPoints` are `NSEvent.deltaX/deltaY`.
    mutating func relative(dxPoints: Double, dyPoints: Double, geometry: InputGeometry) -> (dx: Int32, dy: Int32) {
        let k = (geometry.kx, geometry.ky)
        if let lastK, lastK.x != k.0 || lastK.y != k.1 {
            x.reset()
            y.reset()
        }
        lastK = k
        let delta = geometry.guestDelta(dxPoints: dxPoints, dyPoints: dyPoints)
        return (x.add(delta.dx), y.add(delta.dy))
    }

    /// Section 5.5. `scrollingDeltaX/Y` of AppKit, `precise` = `hasPreciseScrollingDeltas`, `invertedFromDevice` =
    /// `isDirectionInvertedFromDevice` (natural scrolling: the sign is turned back to the device direction).
    /// Wheel: positive = away from the user (HUT 1.7 Wheel). AppKit's positive `scrollingDeltaY` moves the content down, which
    /// is a forward turn of the wheel. Pan: positive = right; AppKit's positive `scrollingDeltaX` moves the content right,
    /// i.e. a tilt to the left, so the sign is turned. Both signs are assumptions checked only by reading, not by a device.
    mutating func scroll(
        deltaX: Double, deltaY: Double, precise: Bool, invertedFromDevice: Bool
    ) -> (wheel: Int32, pan: Int32) {
        let unit = precise ? InputPointerState.wheelUnitsPerPoint : 120
        var wheelUnits = deltaY * unit
        var panUnits = -deltaX * unit
        if invertedFromDevice {
            wheelUnits = -wheelUnits
            panUnits = -panUnits
        }
        return (wheel.add(wheelUnits), pan.add(panUnits))
    }

    mutating func reset() {
        x.reset()
        y.reset()
        wheel.reset()
        pan.reset()
        lastK = nil
    }
}
