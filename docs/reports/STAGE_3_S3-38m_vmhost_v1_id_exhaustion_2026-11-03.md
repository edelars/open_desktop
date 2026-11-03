# Stage 3 S3-38m: V1 RUN-ID exhaustion and fresh helper

Status: focused process acceptance passed; broader V1 acceptance remains open.

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

Independent Sol/high review passed. The smoke uses decoder-supported
`ADD RAX,RBX` for both the maximum-ID invocation and the fresh helper. The
focused process smoke passed on the current ARM64 host against the previously
built, signed VMHost binary, with its exact success marker. Its focused harness
manifest test passed (1/1). No VMHost rebuild was performed for this
fixture-only change. Full process-suite acceptance remains unconfirmed after
an earlier invocation timed out without a summary. Physical Mini acceptance
was not run; the Mini SSH check timed out before any remote command ran. M1
regression remains deferred by the user.

A later `graphify update .` completed successfully and refreshed the project
graph.

## Boundaries

This gate covers the V1 helper-local high-water mark at its maximum value and
fresh-helper initialization only. It does not change VMHost production code,
wire protocol, automatic reconnect/replay, or the S3-38 acceptance scope.
INVALID_STATE batch closure, broader backpressure/cleanup checks, V0
regressions, full process-suite execution, and physical Mini acceptance remain
open.
