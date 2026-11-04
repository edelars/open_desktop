# od-x86-runner

`od-x86-runner --fixture CASE.json --json` executes admitted fixtures on Linux x86-64 in a supervised child process. The v2 tracer includes the integer oracle profiles and the narrow `s2-long-native-xorpd-xmm0-xmm1-v1` SSE slice. Input bytes must pass the project's `od-isa` decoder.

`od-x86-runner --fxsave64-oracle` runs the isolated S2-39k successful CPL3
`48 0F AE 40 40` oracle on Linux x86_64. It does not use fixture/native_v2
admission. Four fresh A/B/A/B observations are written as raw JSON beneath
ignored `work/stage2/s2-39k-fxsave64/`. Other hosts report `unavailable/not_run`.
This mode does not test FXRSTOR, privileged gates/faults, or the synthetic
target MXCSR mask; see the [oracle method](../../docs/testing/S2_39K_FXSAVE64_ORACLE.md).

The schema-v1 trampoline uses guarded code, state and stack mappings; code is writable only while populated, then changed to RX. It saves the host ABI callee-saved registers and captures GPR/RFLAGS after the fixture returns. The v2 tracer instead single-steps a separate tracee and captures registers at the declared stop RIP.

The fixture command caps wall time at the fixture maximum (30 s), CPU time at 2 s, address space at 256 MiB, result stdout at `MAX_RESULT_JSON_BYTES`, and diagnostics stderr at 64 KiB. The reusable supervisor accepts a CPU limit up to 30 s. It kills the whole child process group when limits are exceeded and waits for children. On Linux, `setrlimit` applies CPU, address-space, and core-dump limits before `exec`. Signals remain host harness failures, not guest exceptions.

The v2 tracer reports instruction retirement using ptrace single-step. The historical S2-40b XORPD slice accepts only `66 0F 57 C1`, requires all sixteen XMM registers plus MXCSR `0x1f80`, rejects x87-bearing fixtures, and stops after one instruction. Linux FP-register state is transferred with `PTRACE_GETFPREGS`/`PTRACE_SETFPREGS`; this is OS transport, not guest FXSAVE serialization. S2-40b's physical execution status was `not_run`; the later bounded Linux native acceptance covers only S2-42c/e/h/i/r, with 27 suites unselected. See the [five-suite acceptance](../../docs/testing/S2_42_LINUX_FIVE_SUITE_ACCEPTANCE.md): physical Mac acceptance remains `not_run` and parent S2-42 remains open. Existing x87/SSE and FXSAVE profiles stay denied. Unsupported instruction forms and branch targets are rejected before execution.

## Dependency

- `libc` 0.2.190 from crates.io (`MIT OR Apache-2.0`, default `std` feature): Linux process-limit, ptrace, GPR, and FP-register bindings. This package does not depend on `od-host`.
- `serde_json` 1.0.151 from crates.io (`MIT OR Apache-2.0`, default features): serialize bounded raw S2-39k evidence. The evidence schema and acceptance rules remain defined by this project.
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

The S2-37bv `CVTPS2DQ XMM0,[RAX]` schema-v2 profile is classified local-only;
the portable classifier test and runner's native Unsupported path cover closure.
No profile-specific Linux fixture-denial test or physical native execution is
claimed for this slice.

The S2-37bw `CVTTPD2DQ XMM0,[RAX]` schema-v2 profile is explicitly local-only
and denied by native admission; this slice makes no physical x86 acceptance
claim. This exact instruction profile is not in the native tracer allowlist. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bw--bounded-long64-cvttpd2dq-xmm0rax-aligned-m128-profile).

The S2-37bx `CVTPD2DQ XMM0,[RAX]` schema-v2 profile is explicitly local-only
and remains outside the native tracer allowlist. The portable classifier and
runner Unsupported behavior do not imply native execution or physical x86
acceptance. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bx--bounded-long64-cvtpd2dq-xmm0rax-aligned-m128-profile).

The S2-37by legacy `CVTPS2PD XMM0,[RAX]` profile remains interpreter-only and
is classified local-only for native admission. No native physical execution
or acceptance is claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37by--bounded-long64-cvtps2pd-xmm0rax-m64-memory-profile).

The S2-37bz legacy `CVTSS2SD XMM0,[RAX]` schema-v2 profile is classified
local-only and denied before native execution. The portable classifier and
runner `Unsupported` behavior are closure evidence; no physical x86 execution
is claimed.

S2-37cc CVTSI2SD m32 is a local-only schema-v2 profile. Native admission stays
closed; classifier evidence does not claim physical x86 execution.

S2-37ce CVTSI2SS m32 is classified local-only by the portable native-profile
matcher. The classifier test and runner `Unsupported` result establish backend
closure only; no physical x86 execution or acceptance is claimed.

S2-37cf CVTSI2SS m64 is classified local-only by the portable native profile
matcher. Runner/native classifier denial establishes backend closure only; no
physical x86 execution or acceptance is claimed.

S2-37cj CVTTSD2SI EAX,m64 is classified local-only and remains outside the
native tracer allowlist. The portable classifier and runner Unsupported path
are closure evidence only; no physical x86 execution or acceptance is claimed.

S2-37ck CVTSD2SI RAX,m64 is also classified local-only. Its classifier and
runner denial checks establish backend closure only; no physical x86 execution
or acceptance is claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ck--bounded-long64-cvtsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile).


S2-37cl CVTSS2SI RAX,m32 is classified local-only. Portable classifier and runner denial evidence establishes backend closure only, not physical x86 execution or acceptance.

### S2-37cm local-only classification

The exact legacy `CVTTSS2SI RAX,dword ptr [RAX]` schema-v2 profile remains
classified as local-only. The portable classifier test covers its denial
classification; this is not a claim that native physical execution was run.
