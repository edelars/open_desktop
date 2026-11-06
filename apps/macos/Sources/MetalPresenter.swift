import Foundation
import Metal
import QuartzCore

// Metal presentation for the S10-07 display viewer.
//
// A frame is copied once, from the shared-memory slot into a staging MTLTexture (replace(region:)).
// The staging texture becomes the shown texture only after the reader accepted the frame (generation
// unchanged after the copy, FRAME_TRANSPORT_V1.md 5.2 step 5-6); until then the previously shown
// texture stays untouched. A shown texture that an in-flight command buffer still reads is never
// reused as staging. Pipeline: fullscreen triangle, nearest-neighbour sampling, no blending.
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
    vertex VertexOut od_vertex(uint vertexID [[vertex_id]]) {
        float2 p = float2(float((vertexID << 1) & 2u), float(vertexID & 2u));
        VertexOut out;
        out.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
        out.uv = float2(p.x, 1.0 - p.y);
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

    private let lock = NSLock()
    private var shown: MTLTexture?
    /// Textures that are neither shown nor staged, reused when no in-flight command buffer reads them (S10-10 measures the pool size).
    private var spares: [MTLTexture] = []
    private static let maxSpares = 3
    private var staging: MTLTexture?
    private var inFlight: [ObjectIdentifier: Int] = [:]

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
        lock.unlock()
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

    private func encode(into target: MTLTexture, background: MTLClearColor, commandBuffer: MTLCommandBuffer) {
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = background
        pass.colorAttachments[0].storeAction = .store
        guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: pass) else { return }
        if let texture = retainShown() {
            encoder.setRenderPipelineState(pipeline)
            encoder.setFragmentTexture(texture, index: 0)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            commandBuffer.addCompletedHandler { [weak self] _ in self?.releaseShown(texture) }
        }
        encoder.endEncoding()
    }

    /// Window path: draws the shown texture over the whole drawable, or only the background when nothing is shown.
    func render(to drawable: CAMetalDrawable, background: MTLClearColor) {
        guard let commandBuffer = queue.makeCommandBuffer() else { return }
        encode(into: drawable.texture, background: background, commandBuffer: commandBuffer)
        commandBuffer.present(drawable)
        commandBuffer.commit()
    }

    /// Offscreen path: renders the shown texture 1:1 into a texture of its own size and reads the bytes back.
    /// Returns tightly packed rows (stride = width * 4) in B, G, R, X order, or nil when nothing is shown.
    func renderOffscreen() -> (width: Int, height: Int, pixels: [UInt8])? {
        guard let size = shownSize else { return nil }
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: size.width, height: size.height, mipmapped: false)
        descriptor.usage = .renderTarget
        descriptor.storageMode = .shared
        guard let target = device.makeTexture(descriptor: descriptor),
              let commandBuffer = queue.makeCommandBuffer() else { return nil }
        encode(into: target, background: MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0), commandBuffer: commandBuffer)
        commandBuffer.commit()
        commandBuffer.waitUntilCompleted()
        guard commandBuffer.status == .completed else { return nil }
        var pixels = [UInt8](repeating: 0, count: size.width * size.height * 4)
        pixels.withUnsafeMutableBytes { raw in
            target.getBytes(
                raw.baseAddress!, bytesPerRow: size.width * 4,
                from: MTLRegionMake2D(0, 0, size.width, size.height), mipmapLevel: 0)
        }
        return (size.width, size.height, pixels)
    }
}
