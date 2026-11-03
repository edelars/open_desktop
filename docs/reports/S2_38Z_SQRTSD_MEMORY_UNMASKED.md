# S2-38z — SQRTSD m64 memory with unmasked exceptions

Implemented immutable schema-v2 interpreter-only profile
`s2-long-sqrtsd-memory-rax-unmasked-v1` for exact Long64 `F2 0F 51 00`
(`SQRTSD XMM0,m64 [RAX]`). It requires SSE2 bit 26, echoes its own
`sqrtsd_memory_rax_unmasked_controls`, and reads exactly eight bytes from a
read-only source mapping. The masked S2-37df and register S2-37am profiles
remain separate. Native, JIT, C ABI, generic, and fragment routes stay closed.

The executor reuses the existing scalar SQRTSD raw-u64 semantics without
coupling them to x87. It validates SSE2/EM/OSFXSR and TS gates, MXCSR supported
bits and DAZ, then AC before canonical span checks. Terminal IE takes priority
over DE and PE. Positive subnormal inputs raise DE before computation; `0x1`
produces exact `2^-537` with DE and no PE, while `0x2`/`0x3` are inexact.
Unmasked events select `#XM` or arithmetic `#UD` using the profile's own
OSXMMEXCPT control. Faulting events suppress XMM0, RIP, retirement, and memory
changes; applicable MXCSR sticky flags are retained. Successful execution
preserves XMM0 high64 and unrelated state.

## Coverage

Seven literal fixtures cover masked success and unmasked IE, DE, and PE under
both exception mappings. The runner checks 65,536 cases:

`16 raw-u64 sources × 8 IM/DM/PM patterns × 2 OSXMMEXCPT values × 4 RC values × 64 sticky histories`.

Each case executes the interpreter in-process and runs the schema-v2 result
validator against its independent integer raw-u64 oracle, checking full state,
memory, expected exit, exception metadata, and retirement. Additional tests
cover min-subnormal exactness/inexactness, negative-subnormal and SNaN terminal
IE, high64 preservation, one read8, CPL/gates, AC-before-span, `#GP(0)`, read
failure rollback, control/schema tampering, read-only source enforcement,
interpreter success and JIT/native denial. The MINSD FTZ=0 restriction has a
regression check. Public-API regressions explicitly check missing SSE2, CR0.EM,
and CR4.OSFXSR returning `#UD` before `#NM`, CR0.TS returning `#NM`, full fetch
before those gates, truncated fetch denial, no data callbacks or state changes
on gate failures, and successful CPL 0–3 execution. Validator mutations of
XMM0 low64/high64, RIP, IE/DE/PE sticky bits, retirement, SIMD exception vector,
and the `#GP(0)` error code are all rejected.

The first attempt used the CLI subprocess helper for every generated case and
was stopped after approximately two minutes because of process startup
overhead; it reported no assertion failure. Moving only the matrix harness to
the existing in-process fixture executor preserved the independent oracle and
whole-state validation. The completed matrix took 27.79 seconds.

## Validation

- `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo test -p od-isa --offline`: 1,944 passed after review regressions.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline`: 132 unit tests and 553 fixture tests passed
  after the validator tampering regression.
- The matrix contains 65,536 cases and completed in 27.79 seconds; focused ISA
  callback test confirmed exactly one read8 and rollback behavior.

No physical x86 acceptance is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38z--exact-sqrtsd-memory-profile-with-unmasked-exceptions),
[stage plan](../plans/STAGE_2.md), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38z-sqrtsd-m64-memory-unmasked-exceptions).
