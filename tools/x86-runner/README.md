# od-x86-runner

`od-x86-runner --fixture CASE.json --json` executes admitted fixtures on Linux x86-64 in a supervised child process. The v2 tracer includes the integer oracle profiles and the narrow `s2-long-native-xorpd-xmm0-xmm1-v1` SSE slice. Input bytes must pass the project's `od-isa` decoder.

The schema-v1 trampoline uses guarded code, state and stack mappings; code is writable only while populated, then changed to RX. It saves the host ABI callee-saved registers and captures GPR/RFLAGS after the fixture returns. The v2 tracer instead single-steps a separate tracee and captures registers at the declared stop RIP.

The fixture command caps wall time at the fixture maximum (30 s), CPU time at 2 s, address space at 256 MiB, result stdout at `MAX_RESULT_JSON_BYTES`, and diagnostics stderr at 64 KiB. The reusable supervisor accepts a CPU limit up to 30 s. It kills the whole child process group when limits are exceeded and waits for children. On Linux, `setrlimit` applies CPU, address-space, and core-dump limits before `exec`. Signals remain host harness failures, not guest exceptions.

The v2 tracer reports instruction retirement using ptrace single-step. The S2-40b XORPD slice accepts only `66 0F 57 C1`, requires all sixteen XMM registers plus MXCSR `0x1f80`, rejects x87-bearing fixtures, and stops after one instruction. Linux FP-register state is transferred with `PTRACE_GETFPREGS`/`PTRACE_SETFPREGS`; this is OS transport, not guest FXSAVE serialization. Physical execution remains deferred and `not_run`. Existing x87/SSE and FXSAVE profiles stay denied. Unsupported instruction forms and branch targets are rejected before execution.

## Dependency

- `libc` 0.2.190 from crates.io (`MIT OR Apache-2.0`, default `std` feature): Linux process-limit, ptrace, GPR, and FP-register bindings. This package does not depend on `od-host`.
- `od-isa` from this workspace: strict allowlist decoding of fixture instruction forms and branch targets; the runner does not implement a second decoder.

S2-35.10 PADDSB remains a local interpreter-only XMM register profile. Its literal fixture is explicitly denied by native admission; the S2-40b slice does not add PADDSB native support.

The S2-37bl `CVTTPD2PI MM0,XMM1` profile is explicitly local-only. Native
execution is refused because the native trace does not observe the complete
x87/MMX state required by this fixture.

The S2-37bm `CVTPI2PD XMM0,MM1` profile is explicitly local-only and is refused
before native execution because the trace does not capture the x87/MMX source
state or the complete resulting x87 state.

The S2-37bo `CVTTPS2PI MM0,[RAX]` profile remains local-only: native execution
is refused because the tracer does not capture the complete resulting x87/MMX
state.

The S2-37bp `CVTPS2PI MM0,[RAX]` profile is also local-only. Native execution
is refused because the tracer does not capture the complete resulting x87/MMX
state; this does not constitute physical x86 acceptance.

The S2-37bq `CVTTPD2PI MM0,[RAX]` profile remains interpreter-only. Native
execution is denied because the trace does not capture the complete resulting
x87/MMX state, and this slice makes no physical x86 acceptance claim.

The schema-v2 S2-37br CVTPD2PI MM0,[RAX] profile remains local-only and is
rejected before native execution; no physical x86 acceptance is claimed. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37br--bounded-long64-cvtpd2pi-mm0rax-aligned-m128-memory-profile).

The S2-37bs `CVTDQ2PD XMM0,[RAX]` fixture remains local-only and is denied
before native execution; this slice does not claim physical x86 acceptance.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile).

The S2-37bt `CVTDQ2PS XMM0,[RAX]` profile remains interpreter-only and is
refused before native execution; no physical x86 acceptance is claimed. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile).

The S2-37bu `CVTTPS2DQ XMM0,[RAX]` schema-v2 profile remains interpreter-only
and is denied before native execution; this slice makes no physical x86
acceptance claim. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bu--bounded-long64-cvttps2dq-xmm0rax-aligned-m128-memory-profile).
