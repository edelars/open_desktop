# S2-38an — unmasked SUBPD m128 memory profile

## Implemented scope

The interpreter supports the immutable schema-v2 profile
`s2-long-subpd-memory-unmasked-rax-v1` for exact Long64 `66 0F 5C 00`
(`SUBPD XMM0,m128 [RAX]`). It has separate SSE2/CR0/CR4 and OSXMMEXCPT
controls, keeps the existing masked SUBPD memory and register profiles, reads
one aligned read-only m128, and stops after one instruction at entry RIP+4.
JIT/native and generic routes remain denied.

The profile requires supported MXCSR bits `0xffbf`, reserved bits clear,
DAZ=FTZ=0 and OM=UM=1; it permits all rounding modes, IM/DM/ZM/PM values and
prior sticky flags. Fetch and exact-form checks precede SSE2/EM/OSFXSR `#UD`
and TS `#NM`; a noncanonical fetch span faults first. Data alignment raises
`#GP(0)` regardless of AC, followed by checked canonical span validation and
exactly one read16. Fetch, data-span and read failures leave CPU state and
retirement unchanged.

SUBPD preserves both original raw operands for NaN source selection before
subtraction changes the RHS sign. The first original NaN wins, only a selected
SNaN is quieted, and its sign and payload are retained. IE and lane-local DE
are aggregated before masks. An unmasked pre-event commits applicable IE/DE
only and suppresses post flags, both destination lanes, RIP and retirement.
Post PE delivery commits generated status and suppresses the entire vector
result and retirement. Exact subnormal outputs do not add UE/PE; no inexact-
tiny same-format subtraction case is claimed.

## Verification

- Focused interpreter gate/fetch/CPL/alignment/read-rollback test passed.
- Five literal schema-v2 fixtures passed interpreter result validation,
  control/status/result/exception tampering checks and JIT/native denial.
  The fifth fixture crosses the canonical48 boundary during instruction fetch
  at RIP `0x00007ffffffffffd`; `#GP(0)` takes priority over disabled SSE2.
- The runner executed and validated 1,152 event-product cases plus 393,216
  selected RC/sticky/boundary cases: 394,368 total. All matched the independent
  integer-only raw-u64 oracle using the actual memory RHS. The corrected matrix
  rerun completed in 184.19 seconds on the local Apple Silicon host.
- `cargo test -p od-isa -p od-testkit -p od-runner --offline --no-fail-fast`
  passed: od-isa 1,967; runner unit 148; fixture integration 565; od-testkit
  461. The focused fifth-literal test also passed independently.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all --
  --check` and `git diff --check` passed. Graphify was rebuilt with 12,643
  nodes, 27,358 edges and 403 communities.
- `python3 tools/check_layout.py` reports the linked-worktree `.git` marker as
  unmapped and two pre-existing missing stage1/stage2 oracle manifest links in
  `STAGE_2_REPORT.md`; no S2-38an layout issue was reported.
- Independent Sol/high review passed, including the fetch-span fix and
  #GP(0)-to-#UD(6) exception tampering check.
- No physical x86 acceptance was performed.
