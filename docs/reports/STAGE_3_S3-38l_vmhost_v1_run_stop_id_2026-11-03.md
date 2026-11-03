# Stage 3 S3-38l: active STOP and RUN-ID high-watermark

Status: partial; process smoke and exact harness gate are implemented.

## Scope

One VMHost process accepts RUN ID 1 with code `JNZ -2` and a maximal budget so
the invocation remains active until STOP. It checks the exact response to a
wrong-ID STOP, a duplicate RUN ID, a greater RUN ID rejected as BUSY, and a
decreasing RUN ID. Correct STOP ID 1 must produce `STOP_ACK` before the
terminal `RESULT`, with reason STOP and no claim that retirement must be
positive. The BUSY ID remains consumed: retrying it after the active run drains
must fail as stale. RUN ID 4 then succeeds with new register state, flags, and
budget.

The harness binds the 30-second process step to the exact smoke command,
script fingerprint, marker, and VMHost binary hash. Its fail-closed manifest
self-check covers command, source-hash, and marker drift. No production
protocol behavior was changed.

## Review and execution

Independent Sol/high static reviews of both the process scenario and harness
integration passed. No Python, test, build, formatting, Mini, M1, or process
execution was performed. Mini checks remain deferred until the end of Stage 3;
the separate M1 regression was deferred by the user. The previously completed
S3-39 M1 smoke remains recorded in its existing report.

`graphify update .` was attempted after the code changes, but remained in its
full scan for two minutes. Interrupting it surfaced a `pathlib` traceback, so
the graph refresh could not be confirmed; no Graphify output was changed by
this slice.

## Remaining acceptance

This gate is implemented but unexecuted. V1 INVALID_STATE batch closure,
broader completion/backpressure/cleanup checks, unchanged V0 regressions, and
final runtime acceptance remain open. No performance or runtime claim is made.
