# S2-38ac — ADDSS m32 memory with unmasked exceptions

Implemented the strict schema-v2 interpreter-only profile
`s2-long-addss-memory-unmasked-rax-v1` for exact Long64 `F3 0F 58 00`
(`ADDSS XMM0,m32 [RAX]`). The profile has a separate
`addss_memory_rax_unmasked_controls` echo and leaves the S2-37di masked memory
profile and S2-37bc register behavior intact. It requires SSE bit 25, full
fetch and exact-form checks before SSE/EM/OSFXSR `#UD` and TS `#NM`, supported
MXCSR bits `0xffbf`, reserved bits clear, DAZ=FTZ=0, and OM=UM=1. It admits all
RC modes, IM/DM/ZM/PM masks, and previous sticky histories. AC=1 is refused
locally before address validation; AC=0 allows unaligned memory. The executor
validates canonical48 `[RAX,RAX+3]` before one read4 from a read-only mapping.

Execution uses the existing raw binary32 ADDSS helper. It preserves
XMM0.high96 and all unrelated CPU state on success. SNaNs and opposite-sign
infinities contribute precompute IE; non-NaN subnormal operands contribute
precompute DE; NaNs suppress DE. Unmasked IE/DE commits applicable new
pre-event flags and faults before computation. Otherwise the helper computes
with MXCSR.RC; unmasked PE commits applicable generated status and faults
before destination, RIP, or retirement commit. OSXMMEXCPT selects `#XM` or
arithmetic `#UD`. Overflow and underflow remain masked by profile admission.
Native/JIT/C ABI/generic/fragment paths stay closed.

## Coverage

Three literal fixtures cover masked success, unmasked IE `#XM`, and unmasked
PE arithmetic `#UD`. The runner's independent integer-only binary32 oracle
executes twelve operand/event classes × four RC modes × 64 previous sticky
histories × eight IM/DM/PM mask combinations × both OSXMMEXCPT mappings: 49,152
executions. Classes include directed overflow OE+PE, halfway PE, DE+PE, exact
cancellation, exact tiny subnormal result, opposite infinities, signed SNaNs,
signed QNaNs, QNaN destination plus subnormal memory source (DE suppressed),
and infinity plus subnormal. Each result
checks complete state, status, RIP, retirement, exception mapping, and memory.

Focused ISA tests check full fetch, separate SSE/EM/OSFXSR `#UD` priority over
TS `#NM`, and TS-only `#NM` at CPL0–3. They also check AC-before-span, canonical
span `#GP(0)`, unaligned read at AC=0, one callback, read rollback, pre-event
versus post-event status, XMM suppression, high96 preservation, and successful
retirement. Fixture validation rejects missing or
cross-profile controls, disallowed MXCSR masks, writable/source form changes,
result/control/exception tampering, and non-interpreter backends.

## Validation

- `cargo test -p od-isa --offline --quiet`: 1,954 passed in the isolated
  worktree; the integrated main run passed 1,956.
- `cargo test -p od-testkit --offline --quiet`: 456 passed.
- `cargo test -p od-runner --offline --quiet`: isolated 135 unit/558 fixture;
  integrated main 136 unit/559 fixture tests passed.
- Focused ADDSS matrix: 49,152 cases passed in 20.41 seconds; the final focused
  gate-priority test passed across CPL0–3.
- Focused ISA and fixture tests passed, including the three literal cases.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `python3 tools/check_layout.py`: reports the linked-worktree `.git` pointer
  and the two missing historical oracle manifests already tracked as ISSUE-023.
  The new report and fixture links resolve; no layout/evidence files were
  moved or fabricated.
- Integrated main checks: `cargo fmt --all -- --check`,
  `cargo check --workspace --all-targets --offline`, `python3
  tools/check_layout.py`, and `git diff --check` passed. Main package tests:
  od-isa 1,956, od-testkit 456, runner 136 unit + 559 fixture passed.
- Main `graphify update .`: 12,382 nodes, 26,891 edges, 381 communities;
  community labels were not regenerated without an LLM API key.
- Independent Sol/high code review: clean after numeric-matrix and gate-priority
  coverage fixes.
- A combined `cargo test --workspace --offline --quiet` attempt exceeded the
  context executor's 300-second response limit; its final exit status was not
  captured. All directly affected package suites above passed on integrated
  main.

No physical x86 acceptance is claimed. See the
[CPU contract](../specs/CPU_CONTRACT.md#s2-38ac--exact-addss-memory-profile-with-unmasked-exceptions),
[Stage 2 plan](../plans/STAGE_2.md#s2-38ac--unmasked-outcomes-for-exact-addss-memory-form),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38ac-addss-m32-memory-unmasked-exceptions).
