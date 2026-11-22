import Darwin
import Dispatch
import Foundation
import ODHostABI

// S10-24: test-only fault injection into the writer of the display smoke (`--display-smoke pattern --display-test-fault NAME`).
// It exists to make the failures of docs/testing/DISPLAY.md section 6 happen in a real VMHost process, which tests/system/display_failure_smoke.py
// then observes from outside. It does not change the wire format or the publication algorithm of FRAME_TRANSPORT_V1.md: every
// frame goes through the real `FrameTransportWriter.publishFrame(mode:read:)`, only the source of the pixels and the timing differ.
//
// DEBUG ONLY. The driver is compiled under `#if DEBUG` (a SwiftPM debug build, the one the smokes use); a release build has no
// driver and `DisplayTestFault.isCompiledIn` is false, so `--display-test-fault` is a usage error there (exit status 2), like any
// unknown flag. The shipping VMHost (`swift build -c release`, the application bundle) therefore cannot be driven into these states.
//
// Names (the value after the flag):
//   mid-write:N   the N-th publication (1 or 2) of screen 0 writes the first half of its slot, prints `fault-hook: mid-write ...` to
//                 stderr and then stops forever inside the pixel copy: the slot has `generation = 0` (5.1 steps 2-4) and half a frame,
//                 no FRAME is queued. The smoke kills the process (SIGKILL) at that point. Publication 1 is the first frame of the
//                 connection (the UI has never shown a frame); publication 2 comes after a normal first frame (the UI shows it).
//   flood:N       after the first frame, publishes N further synthetic frames in bursts of 500 per poll tick, whatever the reader does;
//                 every 100th frame starts a new mode (alternating the device mode and its half size). Every pixel of a frame is
//                 `[g, g >> 8, g >> 16, 0]` for its generation `g`, so a frame that mixes two generations is detectable.
//   mode-change   after the first frame and 400 ms, queues MODE with the next `mode_change_count` (half size) without a frame, tries to
//                 publish a frame of the replaced mode (the writer must refuse it, 6.3), waits 800 ms, then publishes the first frame
//                 of the new mode.

/// Parsed in every build (the option parser needs it); acted on only by `DisplayFaultDriver` in a debug build.
enum DisplayTestFault: Equatable {
    case midWrite(publication: Int)
    case flood(frames: Int)
    case modeChange

    static var isCompiledIn: Bool {
        #if DEBUG
        return true
        #else
        return false
        #endif
    }

    static func parse(_ text: String) -> DisplayTestFault? {
        let parts = text.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        switch (parts[0], parts.count) {
        case ("mid-write", 2):
            guard let publication = Int(parts[1]), publication == 1 || publication == 2 else { return nil }
            return .midWrite(publication: publication)
        case ("flood", 2):
            guard let frames = Int(parts[1]), (1...1_000_000).contains(frames) else { return nil }
            return .flood(frames: frames)
        case ("mode-change", 1):
            return .modeChange
        default:
            return nil
        }
    }
}

#if DEBUG
final class DisplayFaultDriver {
    private static let burst = 500
    private static let framesPerMode: UInt64 = 100

    private let fault: DisplayTestFault
    private var phase = 0
    private var deadlineNs: UInt64 = 0
    private var floodPublished = 0
    private var firstModeCount: UInt64 = 0

    init(fault: DisplayTestFault) { self.fault = fault }

    private func log(_ text: String) {
        fputs("od-vmhost-smoke: fault-hook \(text)\n", stderr)
        fflush(stderr)
    }

    private static func nowNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

    private func hangForever() -> Never {
        while true { sleep(1) }
    }

    /// Called once per poll tick instead of the normal publication of the device frame.
    func tick(writer: FrameTransportWriter, session: OpaquePointer, fd: Int32, publishFromDevice: () throws -> Void) throws {
        switch fault {
        case let .midWrite(publication):
            if publication == 2 && writer.lastPublishedGeneration == 0 {
                try publishFromDevice() // the normal first frame; the stall comes at the next tick
                return
            }
            try writer.flush(fd: fd) // MODE (and FRAME 1) are on the socket before the process stops
            try stallInsideSlotWrite(writer: writer, session: session)
        case let .flood(frames):
            try flood(frames: frames, writer: writer, session: session, publishFromDevice: publishFromDevice)
        case .modeChange:
            try modeChange(writer: writer, session: session, publishFromDevice: publishFromDevice)
        }
    }

    private func deviceMode(_ session: OpaquePointer) throws -> od_display_screen_mode {
        try DisplaySmoke.screenMode(session, screen: 0)
    }

    private func plain(width: UInt32, height: UInt32, generation: UInt64, modeCount: UInt64) -> od_display_mode {
        od_display_mode(abi_version: vmhost_display_abi_version_v1(), struct_size: UInt32(OD_DISPLAY_MODE_SIZE_V1),
                        width: width, height: height, stride: width * 4, format: 1, flags: 3, reserved: 0,
                        generation: generation, mode_change_count: modeCount)
    }

    /// Writes `tag` into the pixels of `destination[0..<count]` as the pixel `[g, g >> 8, g >> 16, 0]` (count a multiple of 4).
    private func fill(_ destination: UnsafeMutablePointer<UInt8>, count: Int, tag: UInt64) {
        let pixel: [UInt8] = [UInt8(truncatingIfNeeded: tag), UInt8(truncatingIfNeeded: tag >> 8), UInt8(truncatingIfNeeded: tag >> 16), 0]
        var offset = 0
        while offset + 4 <= count {
            destination[offset] = pixel[0]
            destination[offset + 1] = pixel[1]
            destination[offset + 2] = pixel[2]
            destination[offset + 3] = pixel[3]
            offset += 4
        }
    }

    /// A synthetic frame of generation `generation` through the real publication; true when it was published.
    @discardableResult
    private func publishSynthetic(
        _ writer: FrameTransportWriter, width: UInt32, height: UInt32, generation: UInt64, modeCount: UInt64
    ) throws -> Bool {
        let mode = plain(width: width, height: height, generation: generation, modeCount: modeCount)
        return try writer.publishFrame(mode: mode) { destination, length, info in
            self.fill(destination, count: length, tag: generation)
            info.width = width
            info.height = height
            info.stride = width * 4
            info.format = 1
            info.generation = generation
            info.mode_change_count = modeCount
            return OD_DISPLAY_STATUS_OK.rawValue
        }
    }

    // MARK: mid-write

    /// The publication stops after half of the pixels: the descriptor still says `generation = 0` and no FRAME exists.
    private func stallInsideSlotWrite(writer: FrameTransportWriter, session: OpaquePointer) throws -> Never {
        let mode = try deviceMode(session)
        let generation = writer.lastPublishedGeneration + 1
        let announced = plain(width: mode.width, height: mode.height, generation: generation, modeCount: mode.mode_change_count)
        _ = try writer.publishFrame(mode: announced) { destination, length, info in
            for index in 0..<(length / 2) { destination[index] = 0xA5 }
            self.log("mid-write: first half (\(length / 2) of \(length) bytes) of the pixels of generation \(generation) written, slot generation 0")
            self.hangForever()
        }
        throw FrameTransportError.hostRefusal("mid-write fault did not stall")
    }

    // MARK: flood

    private func flood(
        frames: Int, writer: FrameTransportWriter, session: OpaquePointer, publishFromDevice: () throws -> Void
    ) throws {
        if writer.lastPublishedGeneration == 0 {
            try publishFromDevice()
            if writer.lastPublishedGeneration > 0 { firstModeCount = writer.queuedMode }
            return
        }
        let device = try deviceMode(session)
        let remaining = frames - floodPublished
        if remaining <= 0 { return }
        for _ in 0..<min(Self.burst, remaining) {
            let generation = writer.lastPublishedGeneration + 1
            let serial = (generation - 1) / Self.framesPerMode
            let half = serial % 2 == 1
            let width = half ? device.width / 2 : device.width
            let height = half ? device.height / 2 : device.height
            try publishSynthetic(writer, width: width, height: height, generation: generation, modeCount: firstModeCount + serial)
            floodPublished += 1
        }
        if floodPublished == frames {
            log("flood: \(frames) frames published, last generation \(writer.lastPublishedGeneration), mode_change_count \(writer.queuedMode)")
        }
    }

    // MARK: mode-change

    private func modeChange(writer: FrameTransportWriter, session: OpaquePointer, publishFromDevice: () throws -> Void) throws {
        switch phase {
        case 0:
            try publishFromDevice()
            if writer.lastPublishedGeneration > 0 {
                phase = 1
                deadlineNs = Self.nowNs() + 400_000_000
            }
        case 1 where Self.nowNs() >= deadlineNs:
            let device = try deviceMode(session)
            let oldCount = writer.queuedMode
            let newCount = oldCount + 1
            let width = device.width / 2
            let height = device.height / 2
            try writer.queueMode(count: newCount, active: true, width: width, height: height, stride: width * 4, format: 1)
            log("mode-change: MODE \(newCount) (\(width)x\(height)) queued, no frame of it yet")
            // A frame the device made in the replaced mode must not be published after the MODE (6.3).
            let stale = try publishSynthetic(writer, width: device.width, height: device.height,
                                             generation: writer.lastPublishedGeneration + 1, modeCount: oldCount)
            log("mode-change: frame of the replaced mode \(oldCount) published=\(stale)")
            phase = 2
            deadlineNs = Self.nowNs() + 800_000_000
        case 2 where Self.nowNs() >= deadlineNs:
            let newCount = writer.queuedMode
            let device = try deviceMode(session)
            let published = try publishSynthetic(writer, width: device.width / 2, height: device.height / 2,
                                                 generation: writer.lastPublishedGeneration + 1, modeCount: newCount)
            log("mode-change: first frame of mode \(newCount) published=\(published), generation \(writer.lastPublishedGeneration)")
            phase = 3
        default:
            break
        }
    }
}
#endif
