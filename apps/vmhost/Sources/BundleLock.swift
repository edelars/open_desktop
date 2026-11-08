import Darwin
import Foundation
#if canImport(ODShared)
import ODShared
#endif

// `<bundle>/lock` of a VM bundle: docs/specs/VM_BUNDLE.md section 2.1 (S11-04). Swift over open(2) and flock(2), not a
// function of od-host: there is no C ABI for the lock in od-host (open question 12 of VM_BUNDLE section 11).
//
// Order: `open(O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600)` then `flock(LOCK_EX | LOCK_NB)`; on EEXIST the same
// without O_CREAT | O_EXCL; `fstat` must show a regular file; after the lock `lstat` of the path must give the same
// (st_dev, st_ino). Only EWOULDBLOCK is `busy`. ENOENT at step 2 repeats the whole order once.

enum BundleLockFailure: Error, Equatable, CustomStringConvertible {
    /// `flock` returned EWOULDBLOCK: another open file description holds the lock.
    case busy
    /// `lstat` after `flock` shows another file or none (replaced or removed between `open` and `flock`).
    case lockChanged
    /// The object `lock` is not a regular file (symlink, directory, FIFO, device).
    case invalid
    /// Any other errno (also ENOENT of step 1: there is no bundle directory).
    case io(Int32)

    var description: String {
        switch self {
        case .busy: return "bundle lock is held by another process"
        case .lockChanged: return "bundle lock file was replaced or removed"
        case .invalid: return "bundle lock is not a regular file"
        case .io(let code): return "bundle lock I/O error \(code)"
        }
    }

    /// The `ERROR` code that answers this failure instead of `READY` (CONTROL_PROTOCOL_V2.md section 4 item 2).
    var errorCode: ControlV2ErrorCode {
        switch self {
        case .busy: return .busy
        case .lockChanged: return .lockChanged
        case .invalid: return .lockInvalid
        case .io: return .ioError
        }
    }

    var errnoValue: Int32 {
        if case .io(let code) = self { return code }
        return 0
    }
}

/// The held lock. The descriptor is `O_CLOEXEC`, never passed to a child; the kernel drops the lock when the process dies.
final class BundleLock {
    private(set) var fd: Int32

    private init(fd: Int32) { self.fd = fd }

    deinit { release() }

    /// Releases the lock (closes the descriptor). Idempotent.
    func release() {
        if fd >= 0 {
            Darwin.close(fd)
            fd = -1
        }
    }

    static func acquire(bundlePath: String) throws -> BundleLock {
        let path = bundlePath + "/lock"
        var retried = false
        while true {
            var fd = Darwin.open(path, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0o600)
            if fd < 0 {
                guard errno == EEXIST else { throw BundleLockFailure.io(errno) }
                fd = Darwin.open(path, O_RDWR | O_CLOEXEC | O_NOFOLLOW)
                if fd < 0 {
                    let code = errno
                    if code == ELOOP || code == EISDIR { throw BundleLockFailure.invalid }
                    if code == ENOENT {
                        // The file vanished between the two opens: one repeat of the whole order, then lock_changed.
                        if retried { throw BundleLockFailure.lockChanged }
                        retried = true
                        continue
                    }
                    throw BundleLockFailure.io(code)
                }
            }
            do {
                try verifyAndLock(fd: fd, path: path)
            } catch {
                Darwin.close(fd)
                throw error
            }
            return BundleLock(fd: fd)
        }
    }

    private static func verifyAndLock(fd: Int32, path: String) throws {
        var opened = stat()
        guard fstat(fd, &opened) == 0 else { throw BundleLockFailure.io(errno) }
        guard (opened.st_mode & S_IFMT) == S_IFREG else { throw BundleLockFailure.invalid }
        while true {
            // `flock` is also a struct name in Darwin: the Int32 type of the result selects the function.
            let result: Int32 = flock(fd, LOCK_EX | LOCK_NB)
            if result == 0 { break }
            if errno == EINTR { continue }
            if errno == EWOULDBLOCK { throw BundleLockFailure.busy }
            throw BundleLockFailure.io(errno)
        }
        var named = stat()
        guard lstat(path, &named) == 0 else {
            if errno == ENOENT { throw BundleLockFailure.lockChanged }
            throw BundleLockFailure.io(errno)
        }
        guard named.st_dev == opened.st_dev, named.st_ino == opened.st_ino else { throw BundleLockFailure.lockChanged }
    }
}
