import Darwin
import Foundation
#if canImport(ODAtomics)
import ODAtomics
#endif

// Reader (UI) side of frame transport v3: protocols/control/FRAME_TRANSPORT_V1.md (section 11: one shared memory
// object per screen, one ANNOUNCE per object in screen order, `screen` in MODE, FRAME and CURSOR).
//
// `FrameReader` is the connection: it owns the socket, the inbound byte stream, the descriptor rules of section 4.3
// and the routing of every message to the screen it names. `ScreenReader` is the state of one screen: its mappings,
// its seqlock reads (section 5.2, 10.6), its MODE and FRAME checks and its reader state (section 7.2, 11.7).
//
// Every descriptor field is read exactly once into a local copy, all checks and the copy length use only those locals,
// and a frame or cursor is accepted only when its sequence word is unchanged after the copy.
//
// Atomics go through the shared C shim apps/macos/Sources/ODAtomics/include/FrameTransportAtomics.h (section 5.3): module
// `ODAtomics` in the Swift package, `-import-objc-header` in the direct `xcrun swiftc` builds; the header is not copied.

/// Reader states of section 7.2 for one screen, plus the state of the connection before any ANNOUNCE.
public enum FrameReaderState: Equatable {
    case connecting
    case noSignal
    case modeChanging
    case showing
    /// Screen 1 only: the guest disabled it (DISPLAY_DEVICE.md 16.6). No frame is shown.
    case disabled
    /// EOF, socket error, protocol error, timeout waiting for ANNOUNCE.
    case disconnected(String)
    /// `magic = ODFT` with a foreign version, or an incompatible memory header.
    case incompatible(String)

    public var isTerminal: Bool {
        switch self {
        case .disconnected, .incompatible: return true
        default: return false
        }
    }

    public var name: String {
        switch self {
        case .connecting: return "CONNECTING"
        case .noSignal: return "NO_SIGNAL"
        case .modeChanging: return "MODE_CHANGING"
        case .showing: return "SHOWING"
        case .disabled: return "DISABLED"
        case .disconnected: return "DISCONNECTED"
        case .incompatible: return "INCOMPATIBLE"
        }
    }
}

/// A frame that passed the seqlock check; `pixels` of the copy callback are valid only inside the callback.
public struct AcceptedFrame: Equatable {
    public let generation: UInt64
    public let modeChangeCount: UInt64
    public let width: Int
    public let height: Int
    public let stride: Int
}

/// The cursor state of the last descriptor that passed the seqlock check and the validation (section 10.6).
/// Without a shape (`shapeSeq == 0`) the size, hotspot and `enabled` are zero/false.
public struct AcceptedCursor: Equatable {
    public let cursorSeq: UInt64
    public let enabled: Bool
    public let shapeSeq: UInt64
    public let width: Int
    public let height: Int
    public let hotX: Int
    public let hotY: Int
    public let posX: Int
    public let posY: Int

    public init(cursorSeq: UInt64, enabled: Bool, shapeSeq: UInt64, width: Int, height: Int, hotX: Int, hotY: Int, posX: Int, posY: Int) {
        self.cursorSeq = cursorSeq
        self.enabled = enabled
        self.shapeSeq = shapeSeq
        self.width = width
        self.height = height
        self.hotX = hotX
        self.hotY = hotY
        self.posX = posX
        self.posY = posY
    }
}

/// Called between `g1` and `g2` (section 10.6, step 5) with a pointer to `width * height * 4` bytes of the cursor slot,
/// only when the shape differs from the accepted one. The copy goes to a staging texture that is not used until accepted.
/// Returns false when the copy could not be made (for example no texture could be allocated): the update is then not accepted.
public typealias CursorCopy = (_ pixels: UnsafeRawPointer, _ cursor: AcceptedCursor) -> Bool

/// Called between `g1` and `g2` (section 5.2, step 5) with the screen index, a pointer to `stride * height` bytes of the
/// slot and the frame. The copy must go to private storage that is not shown until the reader accepts the frame.
public typealias FrameCopy = (_ screen: Int, _ pixels: UnsafeRawPointer, _ frame: AcceptedFrame) -> Void

private struct ProtocolFailure: Error {
    let reason: String
    init(_ reason: String) { self.reason = reason }
}

private struct IncompatibleFailure: Error {
    let reason: String
    init(_ reason: String) { self.reason = reason }
}

private struct ConnectionFailure: Error {
    let reason: String
}

private struct ModeMessage {
    let modeChangeCount: UInt64
    let active: Bool
    let disabled: Bool
    let width: Int
    let height: Int
}

/// Wire helpers shared by the connection and the screens.
private enum Wire {
    static func readLE<T: FixedWidthInteger>(_ type: T.Type, _ bytes: [UInt8], _ offset: Int) -> T {
        var value: T = 0
        for index in 0..<MemoryLayout<T>.size {
            value |= T(truncatingIfNeeded: bytes[offset + index]) << T(index * 8)
        }
        return value
    }

    static func appendLE<T: FixedWidthInteger>(_ bytes: inout [UInt8], _ value: T) {
        for index in 0..<MemoryLayout<T>.size {
            bytes.append(UInt8(truncatingIfNeeded: value >> T(index * 8)))
        }
    }

    static func nowMs() -> UInt64 {
        DispatchTime.now().uptimeNanoseconds / 1_000_000
    }
}

/// State of one screen (section 11.6): its object, its mappings, its seqlock reads and its reader state.
public final class ScreenReader {
    // Constants of FRAME_TRANSPORT_V1.md sections 3 and 3.5.
    private static let granule = 16384
    /// Version 3 (section 11): frame slots start at 65536, after the cursor image slots.
    private static let slotsOffset = 65536
    static let messageVersion: UInt16 = 3
    private static let minTotal = 65536 + 2 * 16384
    private static let maxTotal = 65536 + 4 * 37_748_736
    private static let slotSizeMax = 37_748_736
    private static let pixelFormatBGRX8888: UInt32 = 1
    private static let readerClaimOffset = 64
    private static let cursorClaimOffset = 68
    private static let descriptorsOffset = 128
    private static let descriptorSize = 64
    // Cursor area (10.3.2, 10.3.3).
    private static let cursorHeaderOffset = 384
    private static let cursorDescriptorsOffset = 448
    private static let cursorSlotsOffset = 16384
    private static let cursorSlotSize = 16384
    private static let cursorSlotCount = 3
    private static let cursorMaxSide = 64
    private static let cursorFormatBGRA8888Premul: UInt32 = 2

    public let screen: Int
    public private(set) var state: FrameReaderState = .connecting
    /// `shown_once` of section 7.2: true after the first entry into `showing`, never reset.
    private(set) var shownOnce = false
    /// The last accepted frame; nil until the first `showing`.
    public private(set) var lastFrame: AcceptedFrame?
    /// Incremented on every accepted frame; the UI compares it to know that a new frame is ready.
    public private(set) var acceptedCount = 0
    /// The last accepted MODE, for the UI.
    private(set) var modeWidth = 0
    private(set) var modeHeight = 0
    /// The accepted cursor state (section 10.6); screen 0 only. nil until the first accepted CURSOR and after the end.
    public private(set) var cursor: AcceptedCursor?
    /// Incremented on every accepted cursor update.
    public private(set) var cursorAcceptedCount = 0

    public private(set) var announced = false

    // Mappings, valid after a successful ANNOUNCE of this screen.
    private var headerMap: UnsafeMutableRawPointer?
    private var dataMap: UnsafeMutableRawPointer?
    private var dataMapLength = 0

    // Private copy of the validated header (section 3.1).
    private var slotCount = 0
    private var slotSize = 0
    private var maxWidth = 0
    private var maxHeight = 0

    private var lastMode: ModeMessage?
    private var lastFrameNotificationGeneration: UInt64 = 0
    private var sawFrameNotification = false
    private var lastCursorNotificationSeq: UInt64 = 0
    private var sawCursorNotification = false

    init(screen: Int) {
        self.screen = screen
    }

    deinit { unmapAll() }

    /// Releases the mappings of this screen. The reader is terminal afterwards for the connection.
    func unmapAll() {
        if let headerMap { munmap(headerMap, ScreenReader.granule) }
        if let dataMap { munmap(dataMap, dataMapLength) }
        headerMap = nil
        dataMap = nil
        dataMapLength = 0
    }

    /// Sends the connection into a terminal state for this screen; the cursor state dies with the connection (10.6).
    func fail(_ newState: FrameReaderState) {
        state = newState
        cursor = nil
        unmapAll()
    }

    // MARK: ANNOUNCE (section 11.6, 7.1 steps 2-5)

    /// `payload` is the 24-byte ANNOUNCE payload already checked for its own fields by the connection. The
    /// descriptor is closed by this call in every case.
    func accept(payload: [UInt8], descriptor: Int32) throws {
        defer { Darwin.close(descriptor) }
        let totalSize = Wire.readLE(UInt64.self, payload, 0)
        let announcedSlots = Int(Wire.readLE(UInt32.self, payload, 8))
        guard totalSize >= UInt64(ScreenReader.minTotal), totalSize <= UInt64(ScreenReader.maxTotal),
              totalSize % UInt64(ScreenReader.granule) == 0 else {
            throw ProtocolFailure("ANNOUNCE total_size \(totalSize) out of range")
        }
        guard (2...4).contains(announcedSlots) else {
            throw ProtocolFailure("ANNOUNCE slot_count \(announcedSlots) out of range")
        }

        // Step 3: fstat before mmap.
        var info = stat()
        guard fstat(descriptor, &info) == 0 else { throw ProtocolFailure("fstat failed (errno \(errno))") }
        guard info.st_size >= 0, UInt64(info.st_size) == totalSize else {
            throw ProtocolFailure("fstat size \(info.st_size) differs from ANNOUNCE total_size \(totalSize)")
        }
        let total = Int(totalSize)

        // Step 4: header page read-write, data read-only.
        let header = mmap(nil, ScreenReader.granule, PROT_READ | PROT_WRITE, MAP_SHARED, descriptor, 0)
        guard let header, header != MAP_FAILED else { throw ProtocolFailure("mmap of the header page failed (errno \(errno))") }
        let dataLength = total - ScreenReader.granule // includes the cursor image slots (section 10.3)
        let data = mmap(nil, dataLength, PROT_READ, MAP_SHARED, descriptor, off_t(ScreenReader.granule))
        guard let data, data != MAP_FAILED else {
            _ = munmap(header, ScreenReader.granule)
            throw ProtocolFailure("mmap of the slot data failed (errno \(errno))")
        }
        headerMap = header
        dataMap = data
        dataMapLength = dataLength
        odft_fence_acquire()

        // Step 5: validate the header in the order of section 3.5; read every field once.
        do {
            try validateHeader(totalSize: totalSize, announcedSlots: announcedSlots)
        } catch {
            unmapAll()
            throw error
        }
        odft_store_relaxed_u32(
            (header + ScreenReader.readerClaimOffset).assumingMemoryBound(to: UInt32.self), 0)
        announced = true
        state = .noSignal
    }

    private func headerU32(_ offset: Int) -> UInt32 {
        odft_load_relaxed_u32((headerMap! + offset).assumingMemoryBound(to: UInt32.self))
    }

    private func headerU64(_ offset: Int) -> UInt64 {
        odft_load_relaxed_u64((headerMap! + offset).assumingMemoryBound(to: UInt64.self))
    }

    private func validateHeader(totalSize: UInt64, announcedSlots: Int) throws {
        // Fields are read once into locals. The identity fields decide INCOMPATIBLE.
        let magic = headerU32(0)
        let versionAndHeaderSize = headerU32(4)
        let version = UInt16(versionAndHeaderSize & 0xFFFF)
        let headerSize = UInt16(versionAndHeaderSize >> 16)
        let readerBlockOffset = headerU32(8)
        let descriptorsOffset = headerU32(12)
        let descriptorSize = headerU32(16)
        let slots = headerU32(20)
        let slotsOffset = headerU32(24)
        let slotSizeValue = headerU32(28)
        let pixelFormat = headerU32(32)
        let maxW = headerU32(36)
        let maxH = headerU32(40)
        let flags = headerU32(44)
        let headerTotal = headerU64(48)
        let cursorOffset = headerU32(56)
        let screenField = headerU32(60)
        // Cursor area header (10.3.2), every field once.
        let cursorHeader = (0..<8).map { headerU32(ScreenReader.cursorHeaderOffset + 4 * $0) }
        var cursorReserved: UInt64 = 0
        for index in 0..<4 { cursorReserved |= headerU64(ScreenReader.cursorHeaderOffset + 32 + 8 * index) }

        guard magic == 0x5346_444F else { // "ODFS" little-endian
            throw IncompatibleFailure("memory header magic is not ODFS")
        }
        guard version == ScreenReader.messageVersion else {
            throw IncompatibleFailure("memory header version \(version)")
        }
        guard headerSize == 64, readerBlockOffset == 64, descriptorsOffset == 128, descriptorSize == 64,
              slotsOffset == UInt32(ScreenReader.slotsOffset), cursorOffset == UInt32(ScreenReader.cursorHeaderOffset) else {
            throw IncompatibleFailure("memory header layout differs from v3")
        }
        // Section 11.3: the object names its screen; a mismatch with the ANNOUNCE is a protocol error.
        guard screenField == UInt32(screen) else {
            throw ProtocolFailure("memory header screen \(screenField) differs from ANNOUNCE screen \(screen)")
        }
        let expectedCursorHeader: [UInt32] = [
            UInt32(ScreenReader.cursorSlotCount), UInt32(ScreenReader.descriptorSize),
            UInt32(ScreenReader.cursorDescriptorsOffset), UInt32(ScreenReader.cursorSlotsOffset),
            UInt32(ScreenReader.cursorSlotSize), UInt32(ScreenReader.cursorMaxSide),
            UInt32(ScreenReader.cursorMaxSide), ScreenReader.cursorFormatBGRA8888Premul,
        ]
        guard cursorHeader == expectedCursorHeader else {
            throw IncompatibleFailure("cursor area header differs from v3")
        }
        guard pixelFormat == ScreenReader.pixelFormatBGRX8888 else {
            throw IncompatibleFailure("memory header pixel_format \(pixelFormat) is not BGRX8888")
        }
        guard flags == 0 else { throw IncompatibleFailure("memory header flags \(flags)") }

        // Section 3.5 order: ranges first, then products, then slot_size, then total_size in 64 bits.
        guard (2...4).contains(Int(slots)) else { throw ProtocolFailure("memory header slot_count \(slots)") }
        guard (1...4096).contains(Int(maxW)) else { throw ProtocolFailure("memory header max_width \(maxW)") }
        guard (1...2304).contains(Int(maxH)) else { throw ProtocolFailure("memory header max_height \(maxH)") }
        let maxFrame = Int(maxW) * 4 * Int(maxH)
        guard maxFrame <= ScreenReader.slotSizeMax else { throw ProtocolFailure("max frame size out of range") }
        let slotSize = Int(slotSizeValue)
        guard slotSize % ScreenReader.granule == 0, slotSize >= maxFrame, slotSize <= ScreenReader.slotSizeMax else {
            throw ProtocolFailure("memory header slot_size \(slotSizeValue) out of range")
        }
        let expectedTotal = UInt64(ScreenReader.slotsOffset) + UInt64(slots) * UInt64(slotSize)
        guard headerTotal == expectedTotal else { throw ProtocolFailure("memory header total_size does not match its layout") }
        guard headerTotal == totalSize, Int(slots) == announcedSlots else {
            throw ProtocolFailure("memory header disagrees with ANNOUNCE")
        }
        guard cursorReserved == 0 else { throw ProtocolFailure("memory header reserved is not zero") }

        slotCount = Int(slots)
        self.slotSize = slotSize
        maxWidth = Int(maxW)
        maxHeight = Int(maxH)
    }

    // MARK: MODE, FRAME, CURSOR (sections 11.4, 7.3, 10.6)

    /// MODE payload: 32 bytes; flags bit 0 ACTIVE, bit 1 DISABLED (screen 1 only); `screen` at offset 28.
    func handleMode(_ payload: [UInt8]) throws {
        let modeChangeCount = Wire.readLE(UInt64.self, payload, 0)
        let flags = Wire.readLE(UInt32.self, payload, 8)
        let width = Int(Wire.readLE(UInt32.self, payload, 12))
        let height = Int(Wire.readLE(UInt32.self, payload, 16))
        let stride = Int(Wire.readLE(UInt32.self, payload, 20))
        let format = Wire.readLE(UInt32.self, payload, 24)
        let screenValue = Wire.readLE(UInt32.self, payload, 28)
        guard screenValue == UInt32(screen) else { throw ProtocolFailure("MODE screen \(screenValue) on screen \(screen)") }
        let active = flags & 1 != 0
        let disabled = flags & 2 != 0
        guard flags & ~UInt32(3) == 0, !(active && disabled) else { throw ProtocolFailure("MODE flags \(flags) are invalid") }
        guard !disabled || screen == 1 else { throw ProtocolFailure("screen 0 cannot be disabled") }
        if active {
            guard (1...maxWidth).contains(width), (1...maxHeight).contains(height),
                  stride == width * 4, format == ScreenReader.pixelFormatBGRX8888 else {
                throw ProtocolFailure("MODE geometry \(width)x\(height) stride \(stride) format \(format) is invalid")
            }
        } else {
            guard width == 0, height == 0, stride == 0, format == 0 else {
                throw ProtocolFailure("inactive MODE with non-zero geometry")
            }
        }
        if let previous = lastMode, modeChangeCount <= previous.modeChangeCount {
            throw ProtocolFailure("MODE mode_change_count \(modeChangeCount) does not grow")
        }
        lastMode = ModeMessage(modeChangeCount: modeChangeCount, active: active, disabled: disabled,
                               width: width, height: height)
        modeWidth = width
        modeHeight = height
        // Section 7.2: no frame of the new mode yet.
        lastFrame = nil
        if disabled {
            state = .disabled
        } else if !active {
            state = .noSignal
        } else {
            state = shownOnce ? .modeChanging : .noSignal
        }
    }

    /// FRAME payload: 24 bytes; `screen` at offset 20.
    func handleFrame(_ payload: [UInt8]) throws -> (generation: UInt64, modeChangeCount: UInt64, slot: Int) {
        let generation = Wire.readLE(UInt64.self, payload, 0)
        let modeChangeCount = Wire.readLE(UInt64.self, payload, 8)
        let slot = Int(Wire.readLE(UInt32.self, payload, 16))
        let screenValue = Wire.readLE(UInt32.self, payload, 20)
        guard screenValue == UInt32(screen) else { throw ProtocolFailure("FRAME screen \(screenValue) on screen \(screen)") }
        guard generation > 0 else { throw ProtocolFailure("FRAME generation is 0") }
        if sawFrameNotification {
            guard generation > lastFrameNotificationGeneration else {
                throw ProtocolFailure("FRAME generation \(generation) does not grow")
            }
        }
        guard let mode = lastMode else { throw ProtocolFailure("FRAME before any MODE") }
        guard mode.active else { throw ProtocolFailure("FRAME while the mode is inactive") }
        guard modeChangeCount == mode.modeChangeCount else {
            throw ProtocolFailure("FRAME mode_change_count differs from the last MODE")
        }
        guard slot < slotCount else { throw ProtocolFailure("FRAME slot_index \(slot) out of range") }
        sawFrameNotification = true
        lastFrameNotificationGeneration = generation
        return (generation, modeChangeCount, slot)
    }

    /// CURSOR payload: 16 bytes; `screen` at offset 12, which must be 0 (cursors exist for screen 0 only).
    func handleCursor(_ payload: [UInt8]) throws -> (seq: UInt64, slot: Int) {
        let seq = Wire.readLE(UInt64.self, payload, 0)
        let slot = Int(Wire.readLE(UInt32.self, payload, 8))
        let screenValue = Wire.readLE(UInt32.self, payload, 12)
        guard screen == 0, screenValue == 0 else { throw ProtocolFailure("CURSOR for screen \(screenValue)") }
        guard seq > 0 else { throw ProtocolFailure("CURSOR cursor_seq is 0") }
        if sawCursorNotification {
            guard seq > lastCursorNotificationSeq else {
                throw ProtocolFailure("CURSOR cursor_seq \(seq) does not grow")
            }
        }
        guard slot < ScreenReader.cursorSlotCount else { throw ProtocolFailure("CURSOR slot_index \(slot) out of range") }
        sawCursorNotification = true
        lastCursorNotificationSeq = seq
        return (seq, slot)
    }

    // MARK: Cursor read (section 10.6)

    func readCursor(_ notification: (seq: UInt64, slot: Int), copy: CursorCopy) throws {
        guard let header = headerMap, let data = dataMap else { return }
        let slot = notification.slot
        let n = notification.seq
        let descriptor = header + ScreenReader.cursorDescriptorsOffset + ScreenReader.descriptorSize * slot
        let claim = (header + ScreenReader.cursorClaimOffset).assumingMemoryBound(to: UInt32.self)
        let seqPointer = descriptor.assumingMemoryBound(to: UInt64.self)

        // 1. claim the cursor slot (a hint, not a lock).
        odft_store_relaxed_u32(claim, UInt32(slot + 1))
        defer { odft_store_relaxed_u32(claim, 0) } // 7. release the claim

        // 2. g1 with acquire.
        let g1 = odft_load_acquire_u64(seqPointer)
        if g1 != n {
            if g1 != 0 && g1 < n {
                throw ProtocolFailure("cursor slot cursor_seq \(g1) is older than the announced \(n)")
            }
            return // rewritten or being written: discard
        }

        // 3. every descriptor field exactly once into locals.
        func u32(_ offset: Int) -> UInt32 { odft_load_relaxed_u32((descriptor + offset).assumingMemoryBound(to: UInt32.self)) }
        func u64(_ offset: Int) -> UInt64 { odft_load_relaxed_u64((descriptor + offset).assumingMemoryBound(to: UInt64.self)) }
        let shapeSeq = u64(8)
        let flags = u32(16)
        let widthValue = u32(20)
        let heightValue = u32(24)
        let formatValue = u32(28)
        let hotXValue = u32(32)
        let hotYValue = u32(36)
        let posXValue = u32(40)
        let posYValue = u32(44)
        let reservedA = u64(48)
        let reservedB = u64(56)

        // 4. check the locals (width and height before any multiplication; the position against the frame limits
        // of the memory header, not against the 64 pixel image limit).
        let width = Int(widthValue)
        let height = Int(heightValue)
        var valid = (flags == 0 || flags == 1) && reservedA == 0 && reservedB == 0
            && Int(posXValue) < maxWidth && Int(posYValue) < maxHeight
        if shapeSeq == 0 {
            valid = valid && flags == 0 && widthValue == 0 && heightValue == 0 && formatValue == 0
                && hotXValue == 0 && hotYValue == 0
        } else {
            valid = valid && width >= 1 && width <= ScreenReader.cursorMaxSide
                && height >= 1 && height <= ScreenReader.cursorMaxSide
                && formatValue == ScreenReader.cursorFormatBGRA8888Premul
                && Int(hotXValue) < width && Int(hotYValue) < height
        }

        var accepted: AcceptedCursor?
        if valid {
            let candidate = AcceptedCursor(
                cursorSeq: g1, enabled: flags == 1, shapeSeq: shapeSeq, width: width, height: height,
                hotX: Int(hotXValue), hotY: Int(hotYValue), posX: Int(posXValue), posY: Int(posYValue))
            // 5. the image is copied only when the shape differs from the accepted one; not shown before step 6.
            if shapeSeq != 0 && shapeSeq != cursor?.shapeSeq {
                let slotBase = UnsafeRawPointer(data + (ScreenReader.cursorSlotsOffset - ScreenReader.granule)
                    + slot * ScreenReader.cursorSlotSize)
                if copy(slotBase, candidate) {
                    accepted = candidate
                } // else: no image copy, the shape is not taken as accepted; the next CURSOR repeats the copy
            } else {
                accepted = candidate
            }
        }
        // 6. fence(acquire), then g2 with a relaxed load.
        odft_fence_acquire()
        let g2 = odft_load_relaxed_u64(seqPointer)
        if g2 != g1 { return } // torn or overwritten: discard, the accepted state stays
        guard let candidate = accepted else {
            if valid { return } // the copy failed: not a protocol error, the accepted state stays
            throw ProtocolFailure("cursor descriptor violates the protocol while cursor_seq is stable")
        }
        cursor = candidate
        cursorAcceptedCount += 1
    }

    // MARK: Slot read (section 5.2)

    func readSlot(_ notification: (generation: UInt64, modeChangeCount: UInt64, slot: Int),
                  copy: (UnsafeRawPointer, AcceptedFrame) -> Void) throws {
        guard let header = headerMap, let data = dataMap, let mode = lastMode else { return }
        let slot = notification.slot
        let n = notification.generation
        let descriptor = header + ScreenReader.descriptorsOffset + ScreenReader.descriptorSize * slot
        let claim = (header + ScreenReader.readerClaimOffset).assumingMemoryBound(to: UInt32.self)
        let generationPointer = descriptor.assumingMemoryBound(to: UInt64.self)

        // 1. claim the slot (a hint to the writer, not a lock).
        odft_store_relaxed_u32(claim, UInt32(slot + 1))
        defer { odft_store_relaxed_u32(claim, 0) } // 7. release the claim

        // 2. g1 with acquire.
        let g1 = odft_load_acquire_u64(generationPointer)
        if g1 != n {
            if g1 != 0 && g1 < n {
                throw ProtocolFailure("slot generation \(g1) is older than the announced \(n)")
            }
            return // rewritten or being written: discard
        }

        // 3. every descriptor field exactly once into locals.
        let mcc = odft_load_relaxed_u64((descriptor + 8).assumingMemoryBound(to: UInt64.self))
        let widthValue = odft_load_relaxed_u32((descriptor + 16).assumingMemoryBound(to: UInt32.self))
        let heightValue = odft_load_relaxed_u32((descriptor + 20).assumingMemoryBound(to: UInt32.self))
        let strideValue = odft_load_relaxed_u32((descriptor + 24).assumingMemoryBound(to: UInt32.self))
        let formatValue = odft_load_relaxed_u32((descriptor + 28).assumingMemoryBound(to: UInt32.self))
        let stateValue = odft_load_relaxed_u32((descriptor + 32).assumingMemoryBound(to: UInt32.self))
        let reserved0 = odft_load_relaxed_u32((descriptor + 36).assumingMemoryBound(to: UInt32.self))
        let reserved1a = odft_load_relaxed_u64((descriptor + 40).assumingMemoryBound(to: UInt64.self))
        let reserved1b = odft_load_relaxed_u64((descriptor + 48).assumingMemoryBound(to: UInt64.self))
        let reserved1c = odft_load_relaxed_u64((descriptor + 56).assumingMemoryBound(to: UInt64.self))

        // 4. check the locals (width and height against the maxima before any multiplication).
        let width = Int(widthValue)
        let height = Int(heightValue)
        let stride = Int(strideValue)
        let valid = width >= 1 && width <= maxWidth && height >= 1 && height <= maxHeight
            && stride == width * 4 && stride * height <= slotSize
            && formatValue == ScreenReader.pixelFormatBGRX8888 && stateValue == 1
            && reserved0 == 0 && reserved1a == 0 && reserved1b == 0 && reserved1c == 0
            && mcc == notification.modeChangeCount
            && mode.active && width == mode.width && height == mode.height

        var accepted: AcceptedFrame?
        if valid {
            let frame = AcceptedFrame(
                generation: g1, modeChangeCount: mcc, width: width, height: height, stride: stride)
            // 5. the copy uses only the local values; the frame is not shown yet.
            let slotBase = UnsafeRawPointer(data + (ScreenReader.slotsOffset - ScreenReader.granule) + slot * slotSize)
            copy(slotBase, frame)
            accepted = frame
        }
        // 6. fence(acquire), then g2 with a relaxed load.
        odft_fence_acquire()
        let g2 = odft_load_relaxed_u64(generationPointer)
        if g2 != g1 { return } // torn or overwritten: discard
        guard let frame = accepted else {
            throw ProtocolFailure("slot descriptor violates the protocol while the generation is stable")
        }
        lastFrame = frame
        acceptedCount += 1
        shownOnce = true
        state = .showing
    }
}

/// The connection (section 7.1 and 11.4.1): owns the socket, reads the ANNOUNCE of every screen with its descriptor,
/// and routes every message to the screen it names.
public final class FrameReader {
    private static let messageVersion = ScreenReader.messageVersion
    private static let kindVersion: UInt16 = 0
    private static let kindHello: UInt16 = 1
    private static let kindAnnounce: UInt16 = 2
    private static let kindMode: UInt16 = 3
    private static let kindFrame: UInt16 = 4
    private static let kindCursor: UInt16 = 5
    /// Payload lengths of section 11.4 (ANNOUNCE 24, MODE 32, FRAME 24, CURSOR 16).
    private static let payloadLength: [UInt16: Int] = [2: 24, 3: 32, 4: 24, 5: 16]

    private var fd: Int32
    /// Connection state: `connecting` until every screen is announced; terminal states apply to every screen.
    private var connectionState: FrameReaderState = .connecting
    /// One reader per screen; created by the first ANNOUNCE (section 11.4.1), empty before it.
    public private(set) var screens: [ScreenReader] = []

    private var inbound = [UInt8]()
    private var announcedCount = 0
    private var announceDeadline: UInt64 = 0
    private var pendingDescriptor: Int32?

    /// `fd` is the UI end of the socketpair; the connection owns it and closes it in `teardown()`.
    public init(fd: Int32) {
        self.fd = fd
    }

    deinit { teardown() }

    /// Screen 0 (the reader the single-screen UI and the offscreen dump use).
    public var primary: ScreenReader? { screens.first }

    /// Connection state while connecting or terminal; otherwise the state of screen 0 (single-screen callers).
    public var state: FrameReaderState {
        if connectionState == .connecting || connectionState.isTerminal { return connectionState }
        return primary?.state ?? connectionState
    }

    public var isTerminal: Bool { connectionState.isTerminal }

    /// Screen 0 accessors kept for the single-screen callers.
    public var lastFrame: AcceptedFrame? { primary?.lastFrame }
    public var acceptedCount: Int { primary?.acceptedCount ?? 0 }
    public var cursor: AcceptedCursor? { primary?.cursor }
    public var cursorAcceptedCount: Int { primary?.cursorAcceptedCount ?? 0 }

    // MARK: Connection

    /// Section 7.1 step 1: send HELLO. `announceTimeoutMs` bounds the wait for the first ANNOUNCE.
    public func start(announceTimeoutMs: UInt64 = 10_000) {
        guard 16384 % getpagesize() == 0 else {
            fail(.disconnected("host page size \(getpagesize()) does not divide 16384"))
            return
        }
        announceDeadline = Wire.nowMs() + announceTimeoutMs
        var hello = [UInt8]()
        hello.append(contentsOf: Array("ODFT".utf8))
        Wire.appendLE(&hello, FrameReader.messageVersion)
        Wire.appendLE(&hello, FrameReader.kindHello)
        Wire.appendLE(&hello, UInt32(0))
        Wire.appendLE(&hello, UInt32(0))
        var sent = 0
        while sent < hello.count {
            let n = hello.withUnsafeBytes { raw in
                Darwin.send(fd, raw.baseAddress! + sent, hello.count - sent, 0)
            }
            if n > 0 { sent += n; continue }
            if n < 0 && errno == EINTR { continue }
            fail(.disconnected("could not send HELLO (errno \(errno))"))
            return
        }
    }

    /// Releases every mapping and the socket. The connection is terminal afterwards.
    public func teardown() {
        for screen in screens { screen.unmapAll() }
        closePendingDescriptor()
        if fd >= 0 {
            Darwin.close(fd)
            fd = -1
        }
    }

    private func fail(_ newState: FrameReaderState) {
        connectionState = newState
        for screen in screens { screen.fail(newState) }
        teardown()
    }

    private func closePendingDescriptor() {
        if let descriptor = pendingDescriptor {
            Darwin.close(descriptor)
            pendingDescriptor = nil
        }
    }

    /// Bytes the next `recvmsg` may read (section 11.4.1): while ANNOUNCE messages are still due, never past the end
    /// of the current one, so that the descriptor of each ANNOUNCE arrives in the recvmsg of its first byte.
    private var readLimit: Int {
        guard announcedCount < expectedScreens else { return 4096 }
        return inbound.count < 16 ? 16 - inbound.count : 16 + 24 - inbound.count
    }

    /// Screens expected by the connection: 2 until the first ANNOUNCE says otherwise.
    private var expectedScreens: Int { screens.isEmpty ? 2 : screens.count }

    // MARK: Pump

    /// Waits up to `timeoutMs` for the socket, processes every complete message and reads the slots and cursor
    /// named by the last pending notifications. Returns true when a state or a shown frame changed.
    @discardableResult
    public func pump(timeoutMs: Int32, copy: FrameCopy, cursorCopy: CursorCopy = { _, _ in true }) -> Bool {
        if state.isTerminal || fd < 0 { return false }
        let before = state
        let beforeFrames = screens.map(\.lastFrame)
        let beforeCursor = primary?.cursor
        let beforeStates = screens.map(\.state)
        if announcedCount < expectedScreens {
            if Wire.nowMs() >= announceDeadline {
                fail(.disconnected("timeout waiting for ANNOUNCE"))
                return true
            }
        }
        var descriptor = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        let ready = Darwin.poll(&descriptor, 1, timeoutMs)
        if ready < 0 {
            if errno != EINTR { fail(.disconnected("poll failed (errno \(errno))")) }
            return state != before
        }
        if ready == 0 {
            return false
        }
        do {
            try receiveAvailable()
            let pending = try processMessages()
            for (index, frame) in pending.frames.enumerated() {
                if let frame { try screens[index].readSlot(frame) { base, accepted in copy(index, base, accepted) } }
            }
            if let cursorNotification = pending.cursor, let primary {
                try primary.readCursor(cursorNotification, copy: cursorCopy)
            }
        } catch let failure as ProtocolFailure {
            fail(.disconnected("protocol error: \(failure.reason)"))
        } catch let failure as IncompatibleFailure {
            fail(.incompatible(failure.reason))
        } catch let failure as ConnectionFailure {
            fail(.disconnected(failure.reason))
        } catch {
            fail(.disconnected("\(error)"))
        }
        let changed = state != before || screens.map(\.state) != beforeStates
            || screens.map(\.lastFrame) != beforeFrames || primary?.cursor != beforeCursor
        return changed
    }

    // MARK: Receive (section 4.3, 11.4.1)

    private func receiveAvailable() throws {
        let limit = readLimit
        var data = [UInt8](repeating: 0, count: limit)
        // cmsghdr on Darwin: len u32, level i32, type i32, data at offset 12; CMSG_SPACE(4) = 16.
        var control = [UInt8](repeating: 0, count: 16)
        var received = 0
        var controlLength = 0
        var flags: Int32 = 0
        var failedErrno: Int32 = 0
        data.withUnsafeMutableBytes { dataRaw in
            control.withUnsafeMutableBytes { controlRaw in
                var iov = iovec(iov_base: dataRaw.baseAddress, iov_len: dataRaw.count)
                withUnsafeMutablePointer(to: &iov) { iovPointer in
                    var header = msghdr()
                    header.msg_iov = iovPointer
                    header.msg_iovlen = 1
                    header.msg_control = controlRaw.baseAddress
                    header.msg_controllen = socklen_t(controlRaw.count)
                    while true {
                        let n = Darwin.recvmsg(fd, &header, 0)
                        if n < 0 && errno == EINTR { continue }
                        if n < 0 { failedErrno = errno }
                        received = n
                        break
                    }
                    controlLength = Int(header.msg_controllen)
                    flags = header.msg_flags
                }
            }
        }
        if received < 0 { throw ConnectionFailure(reason: "recvmsg failed (errno \(failedErrno))") }

        // Collect descriptors first and mark them close-on-exec before anything else (section 4.3).
        var descriptors: [Int32] = []
        var ancillaryError: String?
        var offset = 0
        let controlEnd = min(controlLength, control.count) // msg_controllen is the real extent of the ancillary data
        while offset + 12 <= controlEnd {
            let length = Int(control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset, as: UInt32.self) })
            let level = control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset + 4, as: Int32.self) }
            let type = control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset + 8, as: Int32.self) }
            if length < 12 || offset + length > controlEnd {
                ancillaryError = "malformed control message"
                break
            }
            if level == SOL_SOCKET && type == SCM_RIGHTS {
                var cursor = offset + 12
                while cursor + 4 <= offset + length {
                    descriptors.append(control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: cursor, as: Int32.self) })
                    cursor += 4
                }
            } else {
                ancillaryError = "unexpected control message level \(level) type \(type)"
            }
            offset += (length + 3) & ~3
        }
        for descriptor in descriptors {
            _ = fcntl(descriptor, F_SETFD, FD_CLOEXEC)
        }
        func closeAll() { for descriptor in descriptors { Darwin.close(descriptor) } }

        if flags & MSG_CTRUNC != 0 {
            closeAll()
            throw ProtocolFailure("MSG_CTRUNC: more descriptors than expected")
        }
        if let ancillaryError {
            closeAll()
            throw ProtocolFailure(ancillaryError)
        }
        if received == 0 {
            closeAll()
            throw ConnectionFailure(reason: "VMHost closed the connection")
        }
        // A descriptor is accepted only in a recvmsg that starts an ANNOUNCE (it returned that message's first byte)
        // and only when no earlier descriptor is waiting for its ANNOUNCE.
        let startsAnnounce = inbound.isEmpty && announcedCount < expectedScreens
        if !descriptors.isEmpty && !(startsAnnounce && pendingDescriptor == nil) {
            closeAll()
            throw ProtocolFailure("descriptor received outside the recvmsg that starts an ANNOUNCE")
        }
        if descriptors.count > 1 {
            closeAll()
            throw ProtocolFailure("more than one descriptor")
        }
        if let first = descriptors.first { pendingDescriptor = first }
        inbound.append(contentsOf: data[0..<received])
    }

    // MARK: Messages (sections 4.1, 4.2, 11.4)

    private struct PendingNotifications {
        /// One pending FRAME per screen, read after the messages of this pass.
        var frames: [(generation: UInt64, modeChangeCount: UInt64, slot: Int)?] = [nil, nil]
        var cursor: (seq: UInt64, slot: Int)?
    }

    /// Processes complete messages; returns the last FRAME of every screen and the last CURSOR that still need a read.
    /// A MODE drops an unread FRAME of its own screen only; a CURSOR is not dropped by a MODE (section 10.6).
    private func processMessages() throws -> PendingNotifications {
        var pending = PendingNotifications()
        while true {
            // Bytes 0-5 first: magic ODFT and version (section 4.1 invariant, 6.1 step 2).
            if inbound.count >= 6 {
                guard inbound[0] == 0x4f, inbound[1] == 0x44, inbound[2] == 0x46, inbound[3] == 0x54 else {
                    closePendingDescriptor()
                    throw ProtocolFailure("bad message magic")
                }
                let version = UInt16(inbound[4]) | UInt16(inbound[5]) << 8
                if version != FrameReader.messageVersion {
                    closePendingDescriptor()
                    throw IncompatibleFailure("incompatible version: VMHost speaks frame transport v\(version), this viewer v\(FrameReader.messageVersion)")
                }
            }
            guard inbound.count >= 16 else { return pending }
            let kind = UInt16(inbound[6]) | UInt16(inbound[7]) << 8
            let reserved = Wire.readLE(UInt32.self, inbound, 8)
            let length = Int(Wire.readLE(UInt32.self, inbound, 12))
            guard let expectedLength = FrameReader.payloadLength[kind] else {
                closePendingDescriptor()
                if kind == FrameReader.kindVersion { throw ProtocolFailure("VERSION with version \(FrameReader.messageVersion)") }
                throw ProtocolFailure("unknown or misdirected message kind \(kind)")
            }
            guard reserved == 0, length == expectedLength else {
                closePendingDescriptor()
                throw ProtocolFailure("message kind \(kind) has a bad header (reserved \(reserved), length \(length))")
            }
            guard inbound.count >= 16 + length else { return pending }
            let payload = Array(inbound[16..<(16 + length)])
            inbound.removeFirst(16 + length)

            if announcedCount < expectedScreens {
                guard kind == FrameReader.kindAnnounce else {
                    closePendingDescriptor()
                    throw ProtocolFailure("message kind \(kind) before every ANNOUNCE")
                }
                try handleAnnounce(payload)
                continue
            }
            // After the last ANNOUNCE a descriptor never arrives; a further ANNOUNCE is an error.
            switch kind {
            case FrameReader.kindAnnounce:
                throw ProtocolFailure("ANNOUNCE after the last screen")
            case FrameReader.kindMode:
                let screenValue = Int(Wire.readLE(UInt32.self, payload, 28))
                try screenOrFail(screenValue).handleMode(payload)
                // A MODE removes an unread FRAME of its own screen.
                pending.frames[screenValue] = nil
            case FrameReader.kindFrame:
                let screenValue = Int(Wire.readLE(UInt32.self, payload, 20))
                let index = try screenIndex(screenValue)
                pending.frames[index] = try screens[index].handleFrame(payload)
            case FrameReader.kindCursor:
                pending.cursor = try screens[0].handleCursor(payload)
            default:
                throw ProtocolFailure("unexpected kind \(kind)")
            }
        }
    }

    private func screenIndex(_ value: Int) throws -> Int {
        guard value >= 0, value < screens.count else { throw ProtocolFailure("screen \(value) is not announced") }
        return value
    }

    private func screenOrFail(_ value: Int) throws -> ScreenReader {
        screens[try screenIndex(value)]
    }

    /// Section 11.4.1 and 11.6 for one ANNOUNCE: the screen index and the screen count are checked first; the
    /// descriptor belongs to this message and is closed by `ScreenReader.accept` in every case.
    private func handleAnnounce(_ payload: [UInt8]) throws {
        guard let descriptor = pendingDescriptor else {
            throw ProtocolFailure("ANNOUNCE without a descriptor")
        }
        pendingDescriptor = nil
        let screenValue = Int(Wire.readLE(UInt32.self, payload, 12))
        let countValue = Int(Wire.readLE(UInt32.self, payload, 16))
        let reservedValue = Wire.readLE(UInt32.self, payload, 20)
        guard reservedValue == 0 else {
            Darwin.close(descriptor)
            throw ProtocolFailure("ANNOUNCE reserved is not zero")
        }
        guard countValue == 1 || countValue == 2 else {
            Darwin.close(descriptor)
            throw ProtocolFailure("ANNOUNCE screen_count \(countValue) out of range")
        }
        guard screens.isEmpty || countValue == screens.count else {
            Darwin.close(descriptor)
            throw ProtocolFailure("ANNOUNCE screen_count differs between ANNOUNCE messages")
        }
        guard screenValue == announcedCount else {
            Darwin.close(descriptor)
            throw ProtocolFailure("ANNOUNCE for screen \(screenValue) out of order (expected \(announcedCount))")
        }
        if screens.isEmpty {
            screens = (0..<countValue).map { ScreenReader(screen: $0) }
        }
        try screens[screenValue].accept(payload: payload, descriptor: descriptor)
        announcedCount += 1
        if announcedCount == screens.count {
            // Every screen is announced: the connection is running; each screen reports its own state from here.
            connectionState = .noSignal
        }
    }
}
