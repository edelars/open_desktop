# S2-37dz: local Long64 memory RSQRTPS XMM0,m128 [RAX]

## Implemented scope

This isolated slice adds exact legacy `0F 52 00` decode and an interpreter-only
schema-v2 profile `s2-long-rsqrtps-memory-rax-v1`. SSE bit 25 is required,
CR0.EM/CR4.OSFXSR faults take #UD priority over CR0.TS #NM after the full code
fetch. The operand requires 16-byte alignment independently of AC, checked
canonical48 start/end addresses, and exactly one read16. All four lanes come
from the memory RHS and replace XMM0. Strict masked MXCSR admission accepts all
RC values and existing sticky combinations while leaving all MXCSR bits
unchanged and adding no flags.

The deterministic reference is `Binary32::reciprocal_sqrt_sse_raw()`. Local
edge policy follows the existing helper for signed zeros/subnormals, negative
normals and infinities, positive infinity, and QNaN/SNaN payload treatment.
The independent normal-output oracle uses the `3/8192` relative-error envelope.
This is not a correctly-rounded or exact physical-hardware match claim. ISSUE-025
remains open and is not modified by this slice. Native/JIT/generic/fragment/C ABI
paths remain denied.

## Validation

Focused ISA/runner checks passed. The focused aggregate included 7 `od-isa`
RSQRTPS tests, 1 strict 4,096-run RC×sticky memory matrix, and 2 runner fixture
checks. Full package suites passed: `od-isa` 1,875; `od-testkit` 455; runner
122 unit tests + 513 fixture tests. The matrix covers both signs at all 23
subnormal leading-bit positions, both signs of QNaN and SNaN with distinct
payloads, signed zeros/infinities, and positive normal boundaries. Workspace
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check` passed. Graphify refreshed to 11,730 nodes, 25,270 edges,
and 381 communities. `tools/check_layout.py` reports only the isolated worktree
`.git` marker and two unavailable historical oracle manifests already recorded
under ISSUE-023; it found no new path/link errors. Independent Sol/high review
is clean; no actionable findings. No physical x86 acceptance is claimed.
