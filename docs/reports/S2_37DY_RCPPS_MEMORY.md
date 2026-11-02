# S2-37dy — Long64 memory RCPPS XMM0,m128 [RAX]

## Scope

The interpreter recognizes exact legacy `0F 53 00` under the separate local
profile `s2-long-rcpps-memory-rax-v1`. It checks SSE bit 25 and CR0.EM/CR4.OSFXSR
before CR0.TS, reads exactly one aligned 16-byte operand after checked canonical48
validation of RAX and RAX+15, and replaces all of XMM0 from four little-endian
memory lanes through `Binary32::reciprocal_sse_raw()`. Alignment remains required
with AC set. The strict memory-profile MXCSR policy admits only supported bits,
all six masks set and DAZ/FTZ clear; every RC/sticky combination is preserved
without adding status bits. Failed gates, address checks, and reads do not commit
CPU state. JIT, native, generic, fragment, and C ABI routes are closed.

## Local ISA checks

The direct interpreter matrix covers all four RC encodings × all 64 old sticky
histories, signed zeros, both infinities, QNaNs/SNaNs with signs and payloads,
every positive and negative subnormal significand-leading position, normal
inputs, and neighbors around the local tiny-result region. The schema-v2 runner
independently checks 14 lane values against integer/rational nearest-even
expectations across 3,584 executions and checks the `3/8192` envelope by integer
cross-products for normal results. The memory source is the sole result source;
a destination poison value and adjacent memory canary detect accidental
dependence on XMM0 or wider/extra reads. Direct checks cover exact decoding and
negative encodings, full instruction fetch before gates, SSE/CR0/CR4/#NM
priority, alignment with AC set, canonical-span failure, MXCSR refusal, one
read16 and rollback on read failure.

The expected bits in this slice intentionally come from S2-36ab's deterministic
reference helper. Its independent integer/rational checks and `3/8192` relative
error envelope for normal outputs remain the evidence for numeric accuracy. No
new physical x86 comparison or threshold inference is made here. ISSUE-025 stays
open; the threshold-overlap region is not claimed to be hardware-bit exact.

## Verification

- `cargo test -p od-isa --offline -q`: 1,870 passed.
- RCPPS-focused decoder and ISA tests: 6 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline -q`: 121 unit and 511 fixture tests passed,
  including the profile/tampering/backend-denial fixture and 3,584-case matrix.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- `python3 tools/check_layout.py`: reported the pre-existing unmapped `.git`
  root pointer and two historical oracle manifest links absent from the
  checkout; it reported no new S2-37dy path or link errors.
- Graphify AST refresh completed: 11,648 nodes, 25,237 edges, 356 communities.

Independent Sol/high review completed clean after correcting the public executor
API comment and using the RCPPS memory control alias in its signature. No physical
x86 acceptance is claimed. ISSUE-025 remains open and the hardware tiny-result
threshold remains unresolved.

## Main integration

Integrated with S2-30dh. Combined main validation passed: od-isa 1,871,
od-testkit 455, runner 121 unit + 512 fixture, and od-x86-runner 8 library + 4
binary tests. Workspace all-target offline check, formatting, layout, and diff
check passed. Main Graphify refresh: 11,660 nodes, 25,258 edges, 368
communities. ISSUE-025 remains open; no hardware bit-equality or physical x86
acceptance is claimed.
