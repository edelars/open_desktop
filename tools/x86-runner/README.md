# od-x86-runner

`od-x86-runner --fixture CASE.json --json` executes admitted fixtures on Linux x86-64 in a supervised child process. The v2 tracer includes the integer oracle profiles and the narrow `s2-long-native-xorpd-xmm0-xmm1-v1` SSE slice. Input bytes must pass the project's `od-isa` decoder.

The schema-v1 trampoline uses guarded code, state and stack mappings; code is writable only while populated, then changed to RX. It saves the host ABI callee-saved registers and captures GPR/RFLAGS after the fixture returns. The v2 tracer instead single-steps a separate tracee and captures registers at the declared stop RIP.

The fixture command caps wall time at the fixture maximum (30 s), CPU time at 2 s, address space at 256 MiB, result stdout at `MAX_RESULT_JSON_BYTES`, and diagnostics stderr at 64 KiB. The reusable supervisor accepts a CPU limit up to 30 s. It kills the whole child process group when limits are exceeded and waits for children. On Linux, `setrlimit` applies CPU, address-space, and core-dump limits before `exec`. Signals remain host harness failures, not guest exceptions.

The v2 tracer reports instruction retirement using ptrace single-step. The S2-40b XORPD slice accepts only `66 0F 57 C1`, requires all sixteen XMM registers plus MXCSR `0x1f80`, rejects x87-bearing fixtures, and stops after one instruction. Linux FP-register state is transferred with `PTRACE_GETFPREGS`/`PTRACE_SETFPREGS`; this is OS transport, not guest FXSAVE serialization. Physical execution remains deferred and `not_run`. Existing x87/SSE and FXSAVE profiles stay denied. Unsupported instruction forms and branch targets are rejected before execution.

## Dependency

- `libc` 0.2.190 from crates.io (`MIT OR Apache-2.0`, default `std` feature): Linux process-limit, ptrace, GPR, and FP-register bindings. This package does not depend on `od-host`.
- `od-isa` from this workspace: strict allowlist decoding of fixture instruction forms and branch targets; the runner does not implement a second decoder.

S2-35.10 PADDSB remains a local interpreter-only XMM register profile. Its literal fixture is explicitly denied by native admission; the S2-40b slice does not add PADDSB native support.
