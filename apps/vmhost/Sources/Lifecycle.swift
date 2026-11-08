import Darwin
import Foundation
#if canImport(ODShared)
import ODShared
#endif
#if canImport(ODHostABI)
import ODHostABI
#endif

// Lifecycle of one machine over control protocol v2 (protocols/control/CONTROL_PROTOCOL_V2.md, S11-04).
//
// This file is the state machine of section 7: states, the table "state x operation" (7.2), the order of checks, request IDs
// and the replay ring (section 5), ATTACH_RESOURCE checks (section 8). It reads no sockets and runs no profile itself: frames
// come in through `handle`, frames to send and the exit status come out, and the profile session is behind `ProfileRunner`.
// Only test-only profiles 1 and 4 exist (no guest, no disks reachable by a guest); nothing here is a statement about a VM.

/// Exit statuses of the VMHost process in the v2 mode (section 12.3).
enum VMHostExitStatus {
    static let ok: Int32 = 0
    static let usage: Int32 = 2
    static let lockRejected: Int32 = 3
    static let protocolError: Int32 = 4
    static let transportLost: Int32 = 5
    static let failed: Int32 = 6
}

/// The machine behind the state machine. For test-only profiles this is a display session that is polled; it is not a guest.
protocol ProfileRunner: AnyObject {
    /// Creates the profile session and everything it needs. A throw means resources are in an unknown state (`failed`).
    func start(profile: UInt32, screens: Int) throws
    /// Stops polling and publishing; the last published frame stays in its slot. Resources are kept.
    func pause()
    /// Starts polling again. `reason` is where the TIME_SYNC hook of S11-22 will look (no guest clock exists yet).
    func resume(reason: ControlV2Reason)
    /// Destroys the session. A throw means resources are in an unknown state (`failed`).
    func stop() throws
}

/// What the I/O loop does after one request.
struct LifecycleOutput {
    /// Frames in the order they must be sent: events first, the terminal frame last (section 5).
    var frames: [ControlV2Frame] = []
    /// Set when the connection ends after the frames are flushed: the exit status of the process.
    var exitStatus: Int32?
}

private struct CompletedRequest {
    let requestID: UInt32
    let kind: ControlV2Kind
    /// The terminal `ACK` or `ERROR`; nil for `QUERY` (a replay of `QUERY` is a fresh `STATE`).
    let response: ControlV2Response?
}

/// Creation of a RAW image in `<bundle>/disks` over the storage C ABI (STORAGE_HOST_ABI_V1.md, S11-07). The mapping of the ABI
/// statuses to `ERROR` codes of CONTROL_PROTOCOL_V2.md section 12.1 follows the section 9 list of answers; the rows that the
/// protocol does not name are marked "no source" in `apps/vmhost/README.md`.
enum DiskImageStorage {
    struct Outcome {
        var error: ControlV2ValueError?
    }

    static var abiVersionMatches: Bool { od_storage_abi_version() == vmhost_storage_abi_version_v1() }

    /// `status` is an `OD_STORAGE_STATUS_*_V1` value; `osErrno`, `required` and `available` are the fields of `od_storage_error_v1`.
    static func map(status: UInt32, osErrno: Int32, required: UInt64, available: UInt64) -> ControlV2ValueError? {
        switch status {
        case UInt32(OD_STORAGE_STATUS_OK_V1):
            return nil
        case UInt32(OD_STORAGE_STATUS_INVALID_ARGUMENT_V1):
            // The only argument the controller chooses is the capacity (section 9: `aux0` = 8); a bad `dir_fd` is `ioError` here.
            if osErrno != 0 { return ControlV2ValueError(.ioError, aux0: UInt64(UInt32(bitPattern: osErrno)), detail: "storage: directory descriptor") }
            return ControlV2ValueError(.invalidArgument, aux0: 8, detail: "capacity_bytes")
        case UInt32(OD_STORAGE_STATUS_INVALID_NAME_V1):
            return ControlV2ValueError(.invalidName, aux0: 0, detail: "disk name")
        case UInt32(OD_STORAGE_STATUS_ALREADY_EXISTS_V1):
            return ControlV2ValueError(.diskExists, detail: "an entry with this name exists; it was not touched")
        case UInt32(OD_STORAGE_STATUS_INSUFFICIENT_SPACE_V1):
            return ControlV2ValueError(.insufficientSpace, aux0: required, aux1: available, detail: "not enough free space for the image")
        case UInt32(OD_STORAGE_STATUS_IO_ERROR_V1):
            return ControlV2ValueError(.ioError, aux0: UInt64(UInt32(bitPattern: osErrno)), detail: "storage I/O error")
        default:
            // Statuses of `inspect` (not_found, not_regular_file, writer_busy, invalid_image, truncated, size_mismatch) and numbers
            // outside the closed list are not answers of `create`: an unexpected ABI result is an I/O error with the number in `detail`.
            // `writer_busy` is not an answer of `create` either (STORAGE_HOST_ABI_V1.md section 7): after an I/O error a file may remain.
            return ControlV2ValueError(.ioError, aux0: UInt64(UInt32(bitPattern: osErrno)), detail: "storage: unexpected status \(status)")
        }
    }

    /// Opens `<bundle>/disks` (`O_DIRECTORY | O_NOFOLLOW`, VM_BUNDLE 2 item 5) and creates the image by the pair "descriptor + name".
    static func createRaw(bundlePath: String, name: [UInt8], capacityBytes: UInt64) -> Outcome {
        let directory = Darwin.open(bundlePath + "/disks", O_DIRECTORY | O_NOFOLLOW | O_RDONLY | O_CLOEXEC)
        guard directory >= 0 else {
            let code = errno
            return Outcome(error: ControlV2ValueError(.ioError, aux0: UInt64(UInt32(bitPattern: code)),
                                                      detail: "open disks/ failed, errno \(code)"))
        }
        defer { Darwin.close(directory) }
        var report = od_storage_error_v1(struct_size: UInt32(OD_STORAGE_ERROR_SIZE_V1), os_errno: 0, required_bytes: 0,
                                         available_bytes: 0, reserved: 0)
        let status = name.withUnsafeBufferPointer { buffer in
            od_storage_create_raw_v1(directory, buffer.baseAddress, UInt32(buffer.count), capacityBytes, 0, &report)
        }
        return Outcome(error: map(status: status, osErrno: report.os_errno, required: report.required_bytes,
                                  available: report.available_bytes))
    }
}

final class Lifecycle {
    /// S9-06 (ISSUE-055) is not closed: every guest `SHUTDOWN` answers `unsupported_durability` (section 7.2), and
    /// `unsupported_no_agent` is unreachable. The constant keeps the order of section 7.2 step 7 visible.
    static let durabilityClosed = false

    private let runner: ProfileRunner
    private let lockFailure: BundleLockFailure?
    private let hostMemoryBytes: UInt64
    /// `--bundle` of the process. `PREPARE_DISK` opens `<bundle>/disks` itself (section 9); nil only in unit tests.
    private let bundlePath: String?

    private(set) var state: ControlV2State = .stopped
    private(set) var configuration: ControlV2Configure?
    private(set) var lastReason: UInt32 = 0
    private var helloSeen = false
    /// `H` of section 5: the largest accepted request ID.
    private(set) var highestRequestID: UInt32 = 0
    private var ring: [CompletedRequest] = []
    /// Descriptors stored by `ATTACH_RESOURCE`: nothing is done with them before S11-08 and later tasks (section 8 item 4).
    private(set) var attached: [UInt16: [UInt16: Int32]] = [:]

    init(runner: ProfileRunner, lockFailure: BundleLockFailure?, hostMemoryBytes: UInt64, bundlePath: String? = nil) {
        self.runner = runner
        self.lockFailure = lockFailure
        self.hostMemoryBytes = hostMemoryBytes
        self.bundlePath = bundlePath
    }

    var attachedDescriptorCount: Int { attached.values.reduce(0) { $0 + $1.count } }

    /// Exit status when the peer closes the connection between frames (section 11): `stopped` is a normal end, the others are a lost transport.
    /// With a failed `lock` an EOF before `HELLO` is status 4, as garbage instead of `HELLO` is (section 4 item 2): the controller
    /// gets no `READY` and no reason. With the lock held an EOF before `HELLO` is an EOF in `stopped` without a request in flight
    /// (status 0): the section 12.3 table does not name this case separately.
    func statusOnEndOfStream() -> Int32 {
        if lockFailure != nil, !helloSeen { return VMHostExitStatus.protocolError }
        return state == .stopped ? VMHostExitStatus.ok : VMHostExitStatus.transportLost
    }

    /// Closes descriptors stored by `ATTACH_RESOURCE`.
    func closeAttachedDescriptors() {
        for (_, byIndex) in attached { for (_, descriptor) in byIndex { Darwin.close(descriptor) } }
        attached.removeAll()
    }

    func stateReport() -> ControlV2StateReport {
        var flags: UInt32 = 0
        if let configuration {
            flags |= ControlV2StateReport.flagConfigured
            if configuration.hasDisks { flags |= ControlV2StateReport.flagDisksNonEmpty }
        }
        return ControlV2StateReport(state: state.rawValue, flags: flags, lastReason: lastReason)
    }

    // MARK: entry

    /// One frame of the controlling process. Throws `ControlV2FatalError`: the connection is closed without a reply (status 4).
    /// A descriptor that arrives with the frame is stored or closed here.
    func handle(_ received: ControlV2Received) throws -> LifecycleOutput {
        let frame = received.frame
        let request: ControlV2Request
        do {
            request = try ControlV2Request.parse(frame)
        } catch {
            closeDescriptor(received.descriptor)
            throw error
        }
        guard helloSeen else {
            closeDescriptor(received.descriptor)
            guard case .hello = request else { throw ControlV2FatalError.helloExpected }
            guard frame.requestID == 0 else { throw ControlV2FatalError.structure("HELLO has request id \(frame.requestID)") }
            helloSeen = true
            return answerHello()
        }
        if case .hello = request {
            closeDescriptor(received.descriptor)
            throw ControlV2FatalError.helloRepeated
        }
        let requestID = frame.requestID
        guard requestID != 0 else {
            closeDescriptor(received.descriptor)
            throw ControlV2FatalError.requestIDZero
        }
        if requestID > highestRequestID, requestID != highestRequestID + 1 {
            closeDescriptor(received.descriptor)
            throw ControlV2FatalError.requestIDSkipped
        }
        if requestID <= highestRequestID {
            closeDescriptor(received.descriptor)
            return replay(requestID: requestID, kind: frame.kind)
        }
        highestRequestID = requestID
        // Every request of this implementation completes before the next frame is read, so a request is never in flight:
        // `in_progress` and `operation_in_progress` (sections 5, 7.2 step 3) are never produced.
        let output = dispatch(request, requestID: requestID, descriptor: received.descriptor)
        if let terminal = output.terminal {
            remember(requestID: requestID, kind: frame.kind, response: terminal)
        } else if frame.kind == .query {
            remember(requestID: requestID, kind: frame.kind, response: nil)
        }
        return output.result
    }

    // MARK: handshake (section 4)

    private func answerHello() -> LifecycleOutput {
        if let lockFailure {
            // Instead of READY: failed_kind 0 (not caused by a request), fatal, the code of the lock outcome (section 4 item 2).
            let error = ControlV2ErrorReport(
                code: lockFailure.errorCode, failedKind: 0, flags: ControlV2ErrorReport.flagFatal, state: state.rawValue,
                aux0: UInt64(UInt32(bitPattern: lockFailure.errnoValue)), detail: "bundle lock: \(lockFailure)")
            return LifecycleOutput(frames: [ControlV2Frame(response: .error(error), requestID: 0)],
                                   exitStatus: VMHostExitStatus.lockRejected)
        }
        return LifecycleOutput(frames: [ControlV2Frame(response: .ready(profile: ControlV2.readyProfile), requestID: 0)])
    }

    // MARK: request IDs (section 5)

    private func remember(requestID: UInt32, kind: ControlV2Kind, response: ControlV2Response?) {
        ring.append(CompletedRequest(requestID: requestID, kind: kind, response: response))
        if ring.count > ControlV2.requestRingSize { ring.removeFirst(ring.count - ControlV2.requestRingSize) }
    }

    /// ID not above `H`: a status request, never a second execution. The state does not change and `H` does not move.
    private func replay(requestID: UInt32, kind: ControlV2Kind) -> LifecycleOutput {
        guard let entry = ring.first(where: { $0.requestID == requestID }) else {
            return LifecycleOutput(frames: [errorFrame(.requestIDStale, kind: kind, requestID: requestID)])
        }
        guard entry.kind == kind else {
            return LifecycleOutput(frames: [errorFrame(.requestIDConflict, kind: kind, requestID: requestID)])
        }
        switch entry.response {
        case .ack(var ack)?:
            ack.flags |= ControlV2Ack.flagReplayed
            return LifecycleOutput(frames: [ControlV2Frame(response: .ack(ack), requestID: requestID)])
        case .error(var error)?:
            error.flags |= ControlV2ErrorReport.flagReplayed
            return LifecycleOutput(frames: [ControlV2Frame(response: .error(error), requestID: requestID)])
        default:
            return LifecycleOutput(frames: [ControlV2Frame(response: .state(stateReport()), requestID: requestID)])
        }
    }

    // MARK: dispatch (section 7.2, order of checks)

    private struct Dispatched {
        var result = LifecycleOutput()
        /// The ACK or ERROR to keep in the ring.
        var terminal: ControlV2Response?
    }

    private func dispatch(_ request: ControlV2Request, requestID: UInt32, descriptor: Int32?) -> Dispatched {
        let kind = request.kind
        func fail(_ error: ControlV2ValueError) -> Dispatched {
            closeDescriptor(descriptor)
            return reject(error, kind: kind, requestID: requestID)
        }
        // Step 4: SET_CLIPBOARD and SUSPEND/SAVE/CHECKPOINT/RESTORE answer `unsupported` in every state.
        switch request {
        case .setClipboard:
            return fail(ControlV2ValueError(.unsupported, detail: "SET_CLIPBOARD arrives with S11-24"))
        case .reservedStage9:
            return fail(ControlV2ValueError(.unsupported, detail: "suspend, save, checkpoint and restore belong to stage 9"))
        default: break
        }
        // Step 5: a non-zero operation ID cannot be recorded before S9-06 (VM_STATE 3.2).
        switch request {
        case .start(let operationID), .stop(let operationID), .shutdown(let operationID):
            if operationID != 0 {
                return fail(ControlV2ValueError(.unsupportedDurability, detail: "operation id needs S9-06"))
            }
        default: break
        }
        // Steps 6 and 7: admissibility in the state, then the checks of the operation.
        switch request {
        case .hello, .setClipboard, .reservedStage9:
            return fail(ControlV2ValueError(.invalidState)) // unreachable: handled above
        case .query:
            var dispatched = Dispatched()
            dispatched.result.frames = [ControlV2Frame(response: .state(stateReport()), requestID: requestID)]
            return dispatched
        case .configure(let value):
            return configure(value, requestID: requestID)
        case .attachResource(let attach):
            return attachResource(attach, descriptor: descriptor, requestID: requestID)
        case .prepareDisk(let prepare):
            return prepareDisk(prepare, requestID: requestID)
        case .start:
            return start(requestID: requestID)
        case .pause(let reason):
            return pause(reason: reason, requestID: requestID)
        case .resume(let reason):
            return resume(reason: reason, requestID: requestID)
        case .stop:
            return stop(requestID: requestID)
        case .shutdown:
            return shutdown(requestID: requestID)
        }
    }

    private func configure(_ value: ControlV2Configure, requestID: UInt32) -> Dispatched {
        guard state == .stopped, configuration == nil else {
            return reject(ControlV2ValueError(.invalidState), kind: .configure, requestID: requestID)
        }
        if let violation = value.validate() { return reject(violation, kind: .configure, requestID: requestID) }
        configuration = value
        return accept(.configure, requestID: requestID)
    }

    /// Section 9: state (step 6), then format, capacity and name, then the image through `od_storage_create_raw_v1` (S11-07). The call is
    /// synchronous and has no progress callback, so the only `PROGRESS` is the final one (`completed == total`) before the `ACK`.
    private func prepareDisk(_ value: ControlV2PrepareDisk, requestID: UInt32) -> Dispatched {
        guard state == .stopped else { return reject(ControlV2ValueError(.invalidState), kind: .prepareDisk, requestID: requestID) }
        if let violation = value.validate() { return reject(violation, kind: .prepareDisk, requestID: requestID) }
        guard let bundlePath else {
            return reject(ControlV2ValueError(.unsupported, detail: "no bundle path: PREPARE_DISK has nowhere to create the image"),
                          kind: .prepareDisk, requestID: requestID)
        }
        guard DiskImageStorage.abiVersionMatches else {
            return failure(kind: .prepareDisk, requestID: requestID, detail: "storage ABI version of libod_host.a does not match the header")
        }
        let outcome = DiskImageStorage.createRaw(bundlePath: bundlePath, name: value.name, capacityBytes: value.capacityBytes)
        if let error = outcome.error { return reject(error, kind: .prepareDisk, requestID: requestID) }
        let progress = ControlV2Frame(response: .progress(requestKind: UInt32(ControlV2Kind.prepareDisk.rawValue),
                                                         completed: value.capacityBytes, total: value.capacityBytes),
                                      requestID: requestID)
        return accept(.prepareDisk, requestID: requestID, after: [progress])
    }

    /// Section 7.3 in the order of VM_BUNDLE 3.5 items 2, 3, 4 and 7.
    private func startChecks(_ configuration: ControlV2Configure) -> ControlV2ValueError? {
        if configuration.hasDisks {
            return ControlV2ValueError(.unsupportedDurability, detail: "a machine with a disk needs S9-06")
        }
        let unsupported: [(Bool, UInt64, String)] = [
            (configuration.network != 0, 12, "network"), (configuration.audio != 0, 13, "audio"),
            (configuration.clipboardSync != 0, 14, "clipboard_sync"), (configuration.bootMedia != 0, 15, "boot_media"),
            (!configuration.sharedFolders.isEmpty, 17, "shared folders"),
        ]
        if let first = unsupported.first(where: { $0.0 }) {
            return ControlV2ValueError(.unsupported, aux0: first.1, detail: "\(first.2) is not available on a test-only profile")
        }
        let limitMiB = hostMemoryBytes / 2 / (1024 * 1024)
        if UInt64(configuration.memoryMiB) > limitMiB {
            return ControlV2ValueError(.hostMemoryExceeded, aux0: UInt64(configuration.memoryMiB), aux1: limitMiB,
                                       detail: "memory_mib is above half of the host memory")
        }
        // Item 4 of section 7.3 (the image is missing) is unreachable while the disk check above answers first.
        return nil
    }

    private func start(requestID: UInt32) -> Dispatched {
        guard state == .stopped else { return reject(ControlV2ValueError(.invalidState), kind: .start, requestID: requestID) }
        guard let configuration else { return reject(ControlV2ValueError(.notConfigured), kind: .start, requestID: requestID) }
        if let violation = startChecks(configuration) { return reject(violation, kind: .start, requestID: requestID) }
        do {
            try runner.start(profile: configuration.machineProfile, screens: Int(configuration.displayScreens))
        } catch {
            return failure(kind: .start, requestID: requestID, detail: "profile session could not be created: \(error)")
        }
        return accept(.start, requestID: requestID, after: transition(to: .running, reason: .user, requestID: requestID))
    }

    private func pause(reason: UInt32, requestID: UInt32) -> Dispatched {
        guard state == .running else { return reject(ControlV2ValueError(.invalidState), kind: .pause, requestID: requestID) }
        guard let pauseReason = ControlV2Reason(rawValue: reason),
              [.user, .systemSleep, .diskSpace].contains(pauseReason) else {
            return reject(ControlV2ValueError(.invalidArgument, aux0: 0, detail: "PAUSE reason"), kind: .pause, requestID: requestID)
        }
        // The safe point is immediate: polling is only a flag of the I/O loop, so `pause_timeout` cannot occur here.
        runner.pause()
        return accept(.pause, requestID: requestID, after: transition(to: .paused, reason: pauseReason, requestID: requestID))
    }

    private func resume(reason: UInt32, requestID: UInt32) -> Dispatched {
        guard state == .paused else { return reject(ControlV2ValueError(.invalidState), kind: .resume, requestID: requestID) }
        guard let resumeReason = ControlV2Reason(rawValue: reason), [.user, .systemWake].contains(resumeReason) else {
            return reject(ControlV2ValueError(.invalidArgument, aux0: 0, detail: "RESUME reason"), kind: .resume, requestID: requestID)
        }
        runner.resume(reason: resumeReason)
        return accept(.resume, requestID: requestID, after: transition(to: .running, reason: resumeReason, requestID: requestID))
    }

    private func stop(requestID: UInt32) -> Dispatched {
        switch state {
        case .stopped:
            if let configuration, configuration.hasDisks {
                return reject(ControlV2ValueError(.unsupportedDurability, detail: "STOP of a machine with a disk needs S9-06"),
                              kind: .stop, requestID: requestID)
            }
            // `stopped` to `stopped` is not a transition: ACK without STATE_CHANGED, then the connection ends (section 6.4).
            var dispatched = accept(.stop, requestID: requestID)
            dispatched.result.exitStatus = VMHostExitStatus.ok
            return dispatched
        case .running, .paused, .shuttingDown:
            do {
                try runner.stop()
            } catch {
                return failure(kind: .stop, requestID: requestID, detail: "profile session could not be destroyed: \(error)")
            }
            var dispatched = accept(.stop, requestID: requestID, after: transition(to: .stopped, reason: .user, requestID: requestID))
            dispatched.result.exitStatus = VMHostExitStatus.ok
            return dispatched
        case .failed:
            return reject(ControlV2ValueError(.invalidState), kind: .stop, requestID: requestID)
        }
    }

    private func shutdown(requestID: UInt32) -> Dispatched {
        guard state == .running else { return reject(ControlV2ValueError(.invalidState), kind: .shutdown, requestID: requestID) }
        // Section 7.2 step 7: durability first (until S9-06 always), the agent only after it.
        if !Self.durabilityClosed || configuration?.hasDisks == true {
            return reject(ControlV2ValueError(.unsupportedDurability, detail: "guest shutdown needs S9-06"), kind: .shutdown,
                          requestID: requestID)
        }
        return reject(ControlV2ValueError(.unsupportedNoAgent, detail: "no guest agent"), kind: .shutdown, requestID: requestID)
    }

    // MARK: ATTACH_RESOURCE (section 8)

    private func attachResource(_ attach: ControlV2Attach, descriptor: Int32?, requestID: UInt32) -> Dispatched {
        func refuse(_ error: ControlV2ValueError) -> Dispatched {
            closeDescriptor(descriptor)
            return reject(error, kind: .attachResource, requestID: requestID)
        }
        guard let descriptor else { return refuse(ControlV2ValueError(.invalidResource)) } // the decoder makes this unreachable
        let role = ControlV2Role(rawValue: attach.role)
        // Admissibility (table 7.2): drop_file only while running or paused, the other roles only before START.
        if role == .dropFile {
            guard state == .running || state == .paused else { return refuse(ControlV2ValueError(.invalidState)) }
        } else {
            guard state == .stopped else { return refuse(ControlV2ValueError(.invalidState)) }
            guard configuration != nil else { return refuse(ControlV2ValueError(.notConfigured)) }
        }
        // 1. Role and index.
        guard let role, let configuration = configuration else {
            return refuse(ControlV2ValueError(.invalidResource, aux0: UInt64(attach.role), detail: "unknown role"))
        }
        let slotIsValid: Bool
        switch role {
        case .bootMedia: slotIsValid = attach.index == 0 && configuration.bootMedia == 1
        case .disk: slotIsValid = Int(attach.index) < configuration.disks.count && configuration.disks[Int(attach.index)].source == 2
        case .sharedFolderRoot: slotIsValid = Int(attach.index) < configuration.sharedFolders.count
        case .dropFile: slotIsValid = attach.index == 0
        }
        guard slotIsValid else {
            return refuse(ControlV2ValueError(.invalidResource, aux0: UInt64(attach.role), detail: "no such resource slot"))
        }
        // 2. A filled slot; the number of drop_file descriptors.
        if role == .dropFile {
            if (attached[attach.role]?.count ?? 0) >= 8 {
                return refuse(ControlV2ValueError(.resourceLimit, detail: "too many drop_file descriptors"))
            }
        } else if attached[attach.role]?[attach.index] != nil {
            return refuse(ControlV2ValueError(.resourceConflict, aux0: UInt64(attach.role), aux1: UInt64(attach.index),
                                              detail: "slot already holds a descriptor"))
        }
        // 3. fstat and the access mode.
        var info = stat()
        guard fstat(descriptor, &info) == 0 else {
            return refuse(ControlV2ValueError(.ioError, aux0: UInt64(UInt32(bitPattern: errno)), detail: "fstat"))
        }
        let type = info.st_mode & S_IFMT
        let typeOK = role == .sharedFolderRoot ? type == S_IFDIR : type == S_IFREG
        guard typeOK else {
            return refuse(ControlV2ValueError(.invalidResource, aux0: UInt64(attach.role), detail: "wrong file type"))
        }
        let flags = fcntl(descriptor, F_GETFL)
        guard flags >= 0 else {
            return refuse(ControlV2ValueError(.ioError, aux0: UInt64(UInt32(bitPattern: errno)), detail: "fcntl"))
        }
        let writableExpected = role == .disk && configuration.disks[Int(attach.index)].writable == 1
        let accessMode = flags & O_ACCMODE
        guard accessMode == (writableExpected ? O_RDWR : O_RDONLY) else {
            return refuse(ControlV2ValueError(.invalidResource, aux0: UInt64(attach.role), detail: "wrong access mode"))
        }
        // 4. The VMHost owns the descriptor until the process ends; nothing uses it yet.
        var slots = attached[attach.role] ?? [:]
        slots[role == .dropFile ? UInt16(slots.count) : attach.index] = descriptor
        attached[attach.role] = slots
        return accept(.attachResource, requestID: requestID)
    }

    /// A failure noticed outside a request (a failed ABI call while polling): `failed`, a fatal `ERROR host_failure` that no request
    /// caused (request ID 0, failed_kind 0), status 6 (sections 5, 11).
    func hostFailure(detail: String) -> LifecycleOutput {
        var frames = transition(to: .failed, reason: .hostFailure, requestID: 0)
        let report = ControlV2ErrorReport(code: .hostFailure, failedKind: 0, flags: ControlV2ErrorReport.flagFatal,
                                          state: state.rawValue, detail: detail)
        frames.append(ControlV2Frame(response: .error(report), requestID: 0))
        return LifecycleOutput(frames: frames, exitStatus: VMHostExitStatus.failed)
    }

    // MARK: frames

    private func closeDescriptor(_ descriptor: Int32?) {
        if let descriptor { Darwin.close(descriptor) }
    }

    private func errorReport(_ code: ControlV2ErrorCode, kind: ControlV2Kind, flags: UInt16 = 0, aux0: UInt64 = 0, aux1: UInt64 = 0,
                             detail: String = "") -> ControlV2ErrorReport {
        ControlV2ErrorReport(code: code, failedKind: kind.rawValue, flags: flags, state: state.rawValue, aux0: aux0, aux1: aux1,
                             detail: detail)
    }

    private func errorFrame(_ code: ControlV2ErrorCode, kind: ControlV2Kind, requestID: UInt32) -> ControlV2Frame {
        ControlV2Frame(response: .error(errorReport(code, kind: kind)), requestID: requestID)
    }

    /// A non-fatal refusal: the state does not change.
    private func reject(_ error: ControlV2ValueError, kind: ControlV2Kind, requestID: UInt32) -> Dispatched {
        let report = errorReport(error.code, kind: kind, aux0: error.aux0, aux1: error.aux1, detail: error.detail)
        var dispatched = Dispatched()
        dispatched.result.frames = [ControlV2Frame(response: .error(report), requestID: requestID)]
        dispatched.terminal = .error(report)
        return dispatched
    }

    private func accept(_ kind: ControlV2Kind, requestID: UInt32, after events: [ControlV2Frame] = []) -> Dispatched {
        let ack = ControlV2Ack(ackedKind: kind.rawValue, state: state.rawValue)
        var dispatched = Dispatched()
        dispatched.result.frames = events + [ControlV2Frame(response: .ack(ack), requestID: requestID)]
        dispatched.terminal = .ack(ack)
        return dispatched
    }

    /// The `STATE_CHANGED` event of a transition; it goes before the terminal frame of the request (section 5).
    private func transition(to next: ControlV2State, reason: ControlV2Reason, requestID: UInt32) -> [ControlV2Frame] {
        let from = state
        state = next
        lastReason = reason.rawValue
        return [ControlV2Frame(response: .stateChanged(from: from.rawValue, to: next.rawValue, reason: reason.rawValue),
                               requestID: requestID)]
    }

    /// `failed` (section 7.1): STATE_CHANGED to `failed`, then a fatal `ERROR host_failure` with the same request ID; status 6.
    private func failure(kind: ControlV2Kind, requestID: UInt32, detail: String) -> Dispatched {
        var frames = transition(to: .failed, reason: .hostFailure, requestID: requestID)
        let report = errorReport(.hostFailure, kind: kind, flags: ControlV2ErrorReport.flagFatal, detail: detail)
        frames.append(ControlV2Frame(response: .error(report), requestID: requestID))
        var dispatched = Dispatched()
        dispatched.result = LifecycleOutput(frames: frames, exitStatus: VMHostExitStatus.failed)
        return dispatched
    }
}
