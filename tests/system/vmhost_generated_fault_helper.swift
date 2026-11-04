import Darwin
import Dispatch
import Foundation

private enum Issue027Trace {
    private static let enabled = ProcessInfo.processInfo.environment["OD_ISSUE027_TRACE"] == "1"

    static func mark(_ phase: String) {
        guard enabled else { return }
        fputs(
            "issue027_trace role=helper uptime_ns=\(DispatchTime.now().uptimeNanoseconds) phase=\(phase)\n",
            stderr
        )
        fflush(stderr)
    }
}

@main
struct GeneratedFaultVMHost {
    static func main() {
        Issue027Trace.mark("helper_started")
        do {
            let descriptor = try parseSocketFD(Array(CommandLine.arguments.dropFirst()))
            Issue027Trace.mark("waiting_hello")
            let hello = try ControlFrame.read(from: descriptor)
            Issue027Trace.mark("hello_received")
            guard hello.kind == .hello, hello.requestID == 0, hello.payload.isEmpty else {
                throw SmokeError.badHandshake
            }

            var readyPayload = Data()
            readyPayload.appendLE(od_host_abi_version())
            try ControlFrame(kind: .ready, requestID: 0, payload: readyPayload).write(to: descriptor)
            Issue027Trace.mark("ready_sent")

            Issue027Trace.mark("waiting_run")
            let run = try ControlFrame.read(from: descriptor)
            Issue027Trace.mark("run_received")
            guard run.kind == .run, run.requestID == 1 else { throw SmokeError.expectedRun }

            Issue027Trace.mark("generated_entry_begin")
            executeGeneratedFault()
            Issue027Trace.mark("generated_entry_returned")
            throw SmokeError.faultReturned
        } catch {
            Issue027Trace.mark("helper_caught_error")
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
        Issue027Trace.mark("jit_mapping_begin")
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
        Issue027Trace.mark("jit_mapping_ready")

        pthread_jit_write_protect_np(0)
        let instructions = mapping.assumingMemoryBound(to: UInt32.self)
        instructions[0] = 0xF940_0000 // ldr x0, [x0]
        instructions[1] = 0xD65F_03C0 // ret
        pthread_jit_write_protect_np(1)
        sys_icache_invalidate(mapping, 2 * MemoryLayout<UInt32>.size)
        Issue027Trace.mark("jit_code_ready")

        Issue027Trace.mark("generated_call_begin")
        let entry = unsafeBitCast(mapping, to: (@convention(c) (UInt64) -> UInt64).self)
        _ = entry(1) // Deliberate invalid host address, expected to terminate this helper.
        Issue027Trace.mark("generated_call_returned")
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
