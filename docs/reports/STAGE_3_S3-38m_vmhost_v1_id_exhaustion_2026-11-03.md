# Stage 3 S3-38m: V1 RUN-ID exhaustion and fresh helper

Status: partial; exact process smoke and harness gate are implemented, but
process execution remains pending.

## Scope

The first VMHost helper accepts HELLO and RUN ID `0xffffffff`, with exact
READY, RUN_ACK, and one-instruction RESULT frames. Repeating the maximum ID
and then sending ID 1 on that same helper must each produce exact scope-1
`INVALID_REQUEST_ID` errors; this checks that IDs do not wrap after exhaustion.
After idle EOF and successful helper exit, a newly launched VMHost with a fresh
socket pair accepts ID 1 with fresh state. The check concerns a fresh helper;
it does not claim an in-place reconnect or automatic RUN replay.

The process harness binds the 30-second command to the source hash, success
marker, and built VMHost SHA-256. Its fail-closed self-check covers manifest
command, source, and marker drift.

## Review and execution

Independent Sol/high static review passed. Python AST parsing and
`git diff --check` passed. The process smoke, harness self-check, VMHost build,
and runtime were not executed. The VMHost build was not available because the
Stage 3 Rust validation blockers in [KNOWN_ISSUES](../KNOWN_ISSUES.md) remain
open. `graphify update .` was attempted after the source changes but stopped
after more than 90 seconds without progress; the graph refresh could not be
confirmed. Mini checks remain deferred until the end of Stage 3, and the
separate M1 regression remains deferred by the user.

## Boundaries

This gate covers the V1 helper-local high-water mark at its maximum value and
fresh-helper initialization only. It does not change VMHost production code,
wire protocol, automatic reconnect/replay, or the S3-38 acceptance scope.
INVALID_STATE batch closure, broader backpressure/cleanup checks, V0
regressions, and final runtime acceptance remain open.
