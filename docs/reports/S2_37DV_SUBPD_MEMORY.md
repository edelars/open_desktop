# S2-37dv — masked-only Long64 SUBPD XMM0,m128 [RAX]

Status: implementation and independent Sol/high review complete; review found no issues.

## Contract

The local schema-v2 interpreter profile accepts only exact legacy `66 0F 5C 00`
in Long64. It requires SSE2, full fetch before #UD/#NM gates, supported masked
MXCSR `0xffbf` with all six masks set and DAZ/FTZ clear. RAX must be 16-byte
aligned and cover a checked canonical48 16-byte span. Execution performs one
atomic 16-byte read, subtracts the actual memory RHS from old XMM0 in two
binary64 lanes, and aggregates IE/DE/OE/UE/PE into old sticky status. JIT,
native, C ABI, generic and fragment routes are closed.

## Evidence

The independent testkit result validator uses `independent_subpd` with the
fixture's memory bytes as the RHS, checks full state/memory/control echoes, and
rejects result/control tampering. The handwritten fixture uses `1.0-2.0` and
`2.0-5.0`. Focused runner coverage includes all RC × 64 sticky histories and
NaN priority, signed zero/cancellation, infinity, subnormal boundaries, ties,
overflow midpoint, mixed-lane IE/DE/OE/PE, #UD/#GP, and backend denial. Direct
ISA tests cover exact decoder neighbors, full fetch before gates, MXCSR refusal
without read, aligned/canonical span checks, generic-path denial, one read16
and read-error rollback.

## Verification

- `cargo test -p od-isa --offline`: 1,856 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- Runner unit suite: 119 passed. Serialized `fixture_v2` suite: 506 passed.
  The SUBPD fixture and the pre-existing ADDPD fixture each passed focused
  reruns after fixing a transient local expectation edit. The SUBPD fixture
  was rerun again after adding schema, WPR and mixed-lane assertions: 1 passed.
- Focused direct ISA test passed; exact SUBPD memory decoder test passed.
- Workspace all-target offline check, formatting check and `git diff --check`
  passed.
- Layout checker reported the linked-worktree `.git` marker plus two existing
  missing oracle manifest links referenced by `STAGE_2_REPORT.md`; no new
  layout path was introduced.
- Graphify AST refresh: 11,596 nodes, 24,979 edges, 364 communities. Semantic
  extraction was unavailable because no API key is configured.

A first runner attempt exposed a temporary signed-zero expectation edit in the
existing ADDPD test while correcting the new SUBPD test. The ADDPD expectation
was restored and its focused test plus the final serialized 506-case fixture
suite passed. Physical x86 acceptance was not run.

Independent Sol/high review was clean at commit `56348bb`; no findings remain.
Integration into main and physical x86 acceptance were not performed in this
worktree.
