import Foundation
import Metal
import QuartzCore
#if canImport(ODShared)
import ODShared
#endif

// Metal presentation for the S10-07 display viewer.
//
// A frame is copied once, from the shared-memory slot into a staging MTLTexture (replace(region:)).
// The staging texture becomes the shown texture only after the reader accepted the frame (generation
// unchanged after the copy, FRAME_TRANSPORT_V1.md 5.2 step 5-6); until then the previously shown
// texture stays untouched. A shown texture that an in-flight command buffer still reads is never
// reused as staging. Pipeline: fullscreen triangle drawn into a viewport (S10-11: aspect-preserving letterbox or
// pixel-exact, geometry in Viewport.swift), nearest-neighbour sampling, no blending.
//
// S10-14 cursor: the cursor image (BGRA8888_PREMUL, bytes copied unchanged into a `bgra8Unorm` texture) is uploaded only when its
// `shape_seq` changes and is drawn as a second draw in the same render pass, after the frame
// (FRAME_TRANSPORT_V1.md 10.8): viewport and texture coordinates from `CursorOverlay.placement`, blending
// `one` / `oneMinusSourceAlpha` on the colour channels, `writeMask` without alpha (the X byte of the frame is not changed).
//
// Byte order: the slot holds B, G, R, X per pixel; MTLPixelFormat.bgra8Unorm stores the same order, so the
// bytes go in unchanged and the X byte lands in the alpha channel without being interpreted.

enum MetalPresenterError: Error, CustomStringConvertible {
    case noDevice
    case setup(String)

    var description: String {
        switch self {
        case .noDevice: return "no Metal device"
        case let .setup(what): return "Metal setup: \(what)"
        }
    }
}

final class MetalPresenter {
    private static let shaderSource = """
    #include <metal_stdlib>
    using namespace metal;

    struct VertexOut {
        float4 position [[position]];
        float2 uv;
    };

    // Fullscreen triangle: vertices (-1,-1), (3,-1), (-1,3); uv (0,1), (2,1), (0,-1), top row of the texture at v = 0.
    // uvRect = (u0, v0, du, dv): the part of the texture shown across the viewport (S10-11, pixel-exact crop).
    vertex VertexOut od_vertex(uint vertexID [[vertex_id]], constant float4 &uvRect [[buffer(0)]]) {
        float2 p = float2(float((vertexID << 1) & 2u), float(vertexID & 2u));
        VertexOut out;
        out.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
        out.uv = uvRect.xy + float2(p.x, 1.0 - p.y) * uvRect.zw;
        return out;
    }

    fragment float4 od_fragment(VertexOut in [[stage_in]], texture2d<float> frame [[texture(0)]]) {
        constexpr sampler nearestSampler(coord::normalized, filter::nearest, address::clamp_to_edge);
        return frame.sample(nearestSampler, in.uv);
    }
    """

    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let pipeline: MTLRenderPipelineState
    /// Same shaders, blending on: the cursor draw (10.8 step 5).
    private let cursorPipeline: MTLRenderPipelineState

    private let lock = NSLock()
    private var shown: MTLTexture?
    /// Textures that are neither shown nor staged, reused when no in-flight command buffer reads them (S10-10 measures the pool size).
    private var spares: [MTLTexture] = []
    private static let maxSpares = 3
    private var staging: MTLTexture?
    private var inFlight: [ObjectIdentifier: Int] = [:]
    /// S10-14: the committed cursor texture (with the `shape_seq` of its bytes), the one being filled, and the accepted state.
    /// Cursor textures are small and allocated per shape change; an in-flight command buffer keeps its texture alive itself.
    private var cursorTexture: (texture: MTLTexture, shapeSeq: UInt64)?
    private var cursorStaging: (texture: MTLTexture, shapeSeq: UInt64)?
    private var cursorState: AcceptedCursor?
    /// Counters for tests and reports: frame texture uploads and cursor texture uploads.
    private(set) var frameUploadCount = 0
    private(set) var cursorUploadCount = 0

    init() throws {
        guard let device = MTLCreateSystemDefaultDevice() else { throw MetalPresenterError.noDevice }
        guard let queue = device.makeCommandQueue() else { throw MetalPresenterError.setup("command queue") }
        self.device = device
        self.queue = queue
        let library: MTLLibrary
        do {
            library = try device.makeLibrary(source: MetalPresenter.shaderSource, options: nil)
        } catch {
            throw MetalPresenterError.setup("shader compilation: \(error)")
        }
        guard let vertex = library.makeFunction(name: "od_vertex"),
              let fragment = library.makeFunction(name: "od_fragment") else {
            throw MetalPresenterError.setup("shader functions")
        }
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = vertex
        descriptor.fragmentFunction = fragment
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        do {
            pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        } catch {
            throw MetalPresenterError.setup("pipeline: \(error)")
        }
        let cursorDescriptor = MTLRenderPipelineDescriptor()
        cursorDescriptor.vertexFunction = vertex
        cursorDescriptor.fragmentFunction = fragment
        guard let attachment = cursorDescriptor.colorAttachments[0] else { throw MetalPresenterError.setup("cursor attachment") }
        attachment.pixelFormat = .bgra8Unorm
        attachment.isBlendingEnabled = true
        attachment.rgbBlendOperation = .add
        attachment.sourceRGBBlendFactor = .one
        attachment.destinationRGBBlendFactor = .oneMinusSourceAlpha
        attachment.alphaBlendOperation = .add
        attachment.sourceAlphaBlendFactor = .zero
        attachment.destinationAlphaBlendFactor = .one
        attachment.writeMask = [.red, .green, .blue] // the alpha channel of the target holds the frame's X byte: untouched
        do {
            cursorPipeline = try device.makeRenderPipelineState(descriptor: cursorDescriptor)
        } catch {
            throw MetalPresenterError.setup("cursor pipeline: \(error)")
        }
    }

    // MARK: Frame upload (reader thread)

    /// Reader thread, inside the seqlock window: copies the slot into a staging texture. The frame is not shown yet.
    func upload(pixels: UnsafeRawPointer, frame: AcceptedFrame) {
        lock.lock()
        var texture: MTLTexture?
        spares.removeAll { $0.width != frame.width || $0.height != frame.height } // other sizes are stale
        if let index = spares.firstIndex(where: { (inFlight[ObjectIdentifier($0)] ?? 0) == 0 }) {
            texture = spares.remove(at: index)
        }
        lock.unlock()
        if texture == nil {
            let descriptor = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: .bgra8Unorm, width: frame.width, height: frame.height, mipmapped: false)
            descriptor.usage = .shaderRead
            descriptor.storageMode = .shared
            texture = device.makeTexture(descriptor: descriptor)
        }
        guard let texture else { return }
        texture.replace(
            region: MTLRegionMake2D(0, 0, frame.width, frame.height),
            mipmapLevel: 0, withBytes: pixels, bytesPerRow: frame.stride)
        lock.lock()
        if let previous = staging { addSpare(previous) }
        staging = texture
        frameUploadCount += 1
        lock.unlock()
    }

    // MARK: Cursor (reader thread uploads, main thread draws)

    /// Reader thread, inside the cursor seqlock window (10.6 step 5): copies `width * height * 4` bytes of the cursor slot into a
    /// new staging texture. Called only for a new `shape_seq`; the texture is not drawn until `commitCursor`.
    /// Returns false when no texture was made (the reader then does not accept the shape and tries again at the next update).
    @discardableResult
    func uploadCursor(pixels: UnsafeRawPointer, cursor: AcceptedCursor) -> Bool {
        guard cursor.width >= 1, cursor.height >= 1 else { return false }
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: cursor.width, height: cursor.height, mipmapped: false)
        descriptor.usage = .shaderRead
        descriptor.storageMode = .shared
        guard let texture = device.makeTexture(descriptor: descriptor) else { return false }
        texture.replace(
            region: MTLRegionMake2D(0, 0, cursor.width, cursor.height),
            mipmapLevel: 0, withBytes: pixels, bytesPerRow: cursor.width * 4)
        lock.lock()
        cursorStaging = (texture, cursor.shapeSeq)
        cursorUploadCount += 1
        lock.unlock()
        return true
    }

    /// The reader accepted a cursor update: its state becomes the drawn state, and the staged texture (if the shape changed)
    /// becomes the cursor texture. The frame texture is not touched.
    func commitCursor(_ state: AcceptedCursor?) {
        lock.lock()
        defer { lock.unlock() }
        guard let state else {
            cursorState = nil
            cursorTexture = nil
            cursorStaging = nil
            return
        }
        if state.shapeSeq == 0 {
            cursorTexture = nil
        } else if let staged = cursorStaging, staged.shapeSeq == state.shapeSeq {
            cursorTexture = staged
        }
        cursorStaging = nil
        // A state whose shape has no texture is never drawn (10.8: shape_seq != 0 and an image).
        cursorState = state.shapeSeq != 0 && cursorTexture?.shapeSeq != state.shapeSeq ? nil : state
    }

    /// The reader discarded the cursor update (g2 != g1): the staged texture is dropped, the drawn state is unchanged.
    func discardCursor() {
        lock.lock()
        cursorStaging = nil
        lock.unlock()
    }

    private func cursorSnapshot() -> (texture: MTLTexture, state: AcceptedCursor)? {
        lock.lock()
        defer { lock.unlock() }
        guard let state = cursorState, state.enabled, state.shapeSeq != 0, let held = cursorTexture,
              held.shapeSeq == state.shapeSeq else { return nil }
        return (held.texture, state)
    }

    /// Caller holds `lock`.
    private func addSpare(_ texture: MTLTexture) {
        spares.append(texture)
        if spares.count > MetalPresenter.maxSpares { spares.removeFirst() }
    }

    /// Reader accepted the frame: the staging texture becomes the shown one.
    func commit() {
        lock.lock()
        if let staged = staging {
            if let old = shown { addSpare(old) }
            shown = staged
            staging = nil
        }
        lock.unlock()
    }

    /// Reader discarded the frame: the staging texture is dropped, the shown one is unchanged.
    func discard() {
        lock.lock()
        if let staged = staging {
            addSpare(staged)
            staging = nil
        }
        lock.unlock()
    }

    /// No frame is current (no signal, mode change, disconnect): nothing is shown as a frame.
    func clear() {
        lock.lock()
        if let old = shown { addSpare(old) }
        shown = nil
        lock.unlock()
    }

    var shownSize: (width: Int, height: Int)? {
        lock.lock()
        defer { lock.unlock() }
        guard let shown else { return nil }
        return (shown.width, shown.height)
    }

    // MARK: Rendering

    private func retainShown() -> MTLTexture? {
        lock.lock()
        defer { lock.unlock() }
        guard let shown else { return nil }
        inFlight[ObjectIdentifier(shown), default: 0] += 1
        return shown
    }

    private func releaseShown(_ texture: MTLTexture) {
        lock.lock()
        let key = ObjectIdentifier(texture)
        let count = (inFlight[key] ?? 1) - 1
        if count <= 0 { inFlight[key] = nil } else { inFlight[key] = count }
        lock.unlock()
    }

    /// Draws the shown texture into `viewport` of the target (nothing when no frame is shown or the viewport is empty).
    /// The target is cleared to `background` first, so the margins around the viewport are plain background.
    /// A viewport that sticks out of the target (pixel-exact mode, small target) is clipped and the frame cropped (see below).
    /// Returns the full (unclipped, possibly negative-origin) viewport of the frame and where the cursor was drawn (nil when
    /// it was not), or nil when no frame was drawn. The cursor is the second draw of the same pass, after the frame, and
    /// only over a drawn frame (10.8).
    @discardableResult
    private func encode(
        into target: MTLTexture, background: MTLClearColor, mode: PresentMode, commandBuffer: MTLCommandBuffer
    ) -> (viewport: Viewport, cursor: CursorPlacement?)? {
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = background
        pass.colorAttachments[0].storeAction = .store
        guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: pass) else { return nil }
        var drawn: (viewport: Viewport, cursor: CursorPlacement?)?
        if let texture = retainShown() {
            let viewport = ViewportLayout.viewport(
                mode: mode, frameWidth: texture.width, frameHeight: texture.height,
                targetWidth: target.width, targetHeight: target.height)
            // The viewport handed to Metal never leaves the render pass: it is clipped to the target and the
            // texture coordinates are narrowed to the part of the frame that stays visible.
            let visible = viewport.clipped(targetWidth: target.width, targetHeight: target.height)
            if visible.isEmpty {
                releaseShown(texture)
            } else {
                let uv = ViewportLayout.visibleUV(full: viewport, visible: visible)
                var uvRect = SIMD4<Float>(Float(uv.u0), Float(uv.v0), Float(uv.du), Float(uv.dv))
                encoder.setViewport(MTLViewport(
                    originX: Double(visible.x), originY: Double(visible.y),
                    width: Double(visible.width), height: Double(visible.height), znear: 0, zfar: 1))
                encoder.setVertexBytes(&uvRect, length: MemoryLayout<SIMD4<Float>>.size, index: 0)
                encoder.setRenderPipelineState(pipeline)
                encoder.setFragmentTexture(texture, index: 0)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
                commandBuffer.addCompletedHandler { [weak self] _ in self?.releaseShown(texture) }
                var placed: CursorPlacement?
                if let cursor = cursorSnapshot(),
                   let placement = CursorOverlay.placement(
                       frameWidth: texture.width, frameHeight: texture.height, posX: cursor.state.posX,
                       posY: cursor.state.posY, hotX: cursor.state.hotX, hotY: cursor.state.hotY,
                       width: cursor.state.width, height: cursor.state.height, full: viewport, visible: visible) {
                    // The viewport of the cursor is inside `visible`, so inside the render target; the scissor rect is a safety net.
                    var cursorUV = SIMD4<Float>(
                        Float(placement.u0), Float(placement.v0), Float(placement.du), Float(placement.dv))
                    encoder.setScissorRect(MTLScissorRect(x: visible.x, y: visible.y, width: visible.width, height: visible.height))
                    encoder.setViewport(MTLViewport(
                        originX: placement.x0, originY: placement.y0, width: placement.x1 - placement.x0,
                        height: placement.y1 - placement.y0, znear: 0, zfar: 1))
                    encoder.setVertexBytes(&cursorUV, length: MemoryLayout<SIMD4<Float>>.size, index: 0)
                    encoder.setRenderPipelineState(cursorPipeline)
                    encoder.setFragmentTexture(cursor.texture, index: 0)
                    encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
                    placed = placement
                }
                drawn = (viewport, placed)
            }
        }
        encoder.endEncoding()
        return drawn
    }

    /// Window path: draws the shown texture into the drawable (aspect-preserving or pixel-exact), margins are `background`.
    /// A resize only changes the drawable and the viewport; the shown texture is not touched or recreated.
    @discardableResult
    func render(to drawable: CAMetalDrawable, background: MTLClearColor, mode: PresentMode) -> Viewport? {
        guard let commandBuffer = queue.makeCommandBuffer() else { return nil }
        let drawn = encode(into: drawable.texture, background: background, mode: mode, commandBuffer: commandBuffer)
        commandBuffer.present(drawable)
        commandBuffer.commit()
        return drawn?.viewport
    }

    /// Offscreen path: renders the shown texture into a `targetWidth` x `targetHeight` texture (physical pixels) and reads
    /// the bytes back. With the frame size as target in `.fit` mode this is the 1:1 dump (nearest sampling, no filtering).
    /// Returns tightly packed rows (stride = targetWidth * 4) in B, G, R, X order and the viewport that was drawn,
    /// or nil when nothing is shown. Margins are B, G, R, X = 0.
    func renderOffscreen(
        targetWidth: Int, targetHeight: Int, mode: PresentMode
    ) -> (width: Int, height: Int, pixels: [UInt8], viewport: Viewport, cursor: CursorPlacement?)? {
        guard targetWidth > 0, targetHeight > 0 else { return nil }
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: targetWidth, height: targetHeight, mipmapped: false)
        descriptor.usage = .renderTarget
        descriptor.storageMode = .shared
        guard let target = device.makeTexture(descriptor: descriptor),
              let commandBuffer = queue.makeCommandBuffer() else { return nil }
        let drawn = encode(
            into: target, background: MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0), mode: mode,
            commandBuffer: commandBuffer)
        commandBuffer.commit()
        commandBuffer.waitUntilCompleted()
        guard commandBuffer.status == .completed, let viewport = drawn else { return nil }
        var pixels = [UInt8](repeating: 0, count: targetWidth * targetHeight * 4)
        pixels.withUnsafeMutableBytes { raw in
            target.getBytes(
                raw.baseAddress!, bytesPerRow: targetWidth * 4,
                from: MTLRegionMake2D(0, 0, targetWidth, targetHeight), mipmapLevel: 0)
        }
        return (targetWidth, targetHeight, pixels, viewport.viewport, viewport.cursor)
    }
}
