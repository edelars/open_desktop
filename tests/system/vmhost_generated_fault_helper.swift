import Darwin
import Foundation

@main
struct GeneratedFaultVMHost {
    static func main() {
        do {
            let descriptor = try parseSocketFD(Array(CommandLine.arguments.dropFirst()))
            let hello = try ControlFrame.read(from: descriptor)
            guard hello.kind == .hello, hello.requestID == 0, hello.payload.isEmpty else {
                throw SmokeError.badHandshake
            }

            var readyPayload = Data()
            readyPayload.appendLE(od_host_abi_version())
            try ControlFrame(kind: .ready, requestID: 0, payload: readyPayload).write(to: descriptor)

            let run = try ControlFrame.read(from: descriptor)
            guard run.kind == .run, run.requestID == 1 else { throw SmokeError.expectedRun }

            executeGeneratedFault()
            throw SmokeError.faultReturned
        } catch {
            fputs("vmhost-generated-fault: \(error)\n", stderr)
            exit(EXIT_FAILURE)
        }
    }

    private static func parseSocketFD(_ arguments: [String]) throws -> Int32 {
        guard arguments.count == 2,
              arguments[0] == "--socket-fd",
              let descriptor = Int32(arguments[1]), descriptor >= 0
        else {
            throw SmokeError.usage
        }
        return descriptor
    }

    private static func executeGeneratedFault() {
        let pageSize = Int(getpagesize())
        guard let mapping = mmap(
            nil,
            pageSize,
            PROT_READ | PROT_WRITE | PROT_EXEC,
            MAP_PRIVATE | MAP_ANON | MAP_JIT,
            -1,
            0
        ), mapping != MAP_FAILED else {
            fatalError("MAP_JIT allocation failed")
        }

        pthread_jit_write_protect_np(0)
        let instructions = mapping.assumingMemoryBound(to: UInt32.self)
        instructions[0] = 0xF940_0000 // ldr x0, [x0]
        instructions[1] = 0xD65F_03C0 // ret
        pthread_jit_write_protect_np(1)
        sys_icache_invalidate(mapping, 2 * MemoryLayout<UInt32>.size)

        let entry = unsafeBitCast(mapping, to: (@convention(c) (UInt64) -> UInt64).self)
        _ = entry(1) // Deliberate invalid host address, expected to terminate this helper.
    }
}

private enum SmokeError: Error, CustomStringConvertible {
    case usage
    case badHandshake
    case expectedRun
    case faultReturned

    var description: String {
        switch self {
        case .usage: return "usage: helper --socket-fd FD"
        case .badHandshake: return "launcher handshake was invalid"
        case .expectedRun: return "launcher did not send RUN"
        case .faultReturned: return "deliberate generated-code fault unexpectedly returned"
        }
    }
}
