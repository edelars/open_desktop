# Stage 3 S3-38l: active STOP and RUN-ID high-watermark

Status: focused process acceptance passed; broader V1 acceptance remains open.

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

Independent Sol/high static reviews of the process scenario and harness
integration passed. The focused process smoke passed on the current ARM64 host
against the previously built, signed VMHost binary with the exact success
marker. Its focused manifest test passed (1/1). Full process-suite acceptance
remains unconfirmed after an earlier run timed out without a summary; physical
Mini acceptance was not run. The Mini SSH check timed out before any remote
command ran. M1 regression remains deferred by the user; the previously
completed S3-39 M1 smoke remains recorded in its existing report.

The fixture was not changed in this acceptance pass, so no build or formatting
check was needed. A later `graphify update .` completed successfully and
refreshed the project graph.

## Remaining acceptance

V1 INVALID_STATE batch closure, broader completion/backpressure/cleanup checks,
unchanged V0 regressions, full process-suite execution, and physical Mini
acceptance remain open. This report makes no performance claim.
