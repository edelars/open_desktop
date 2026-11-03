# Stage 3 S3-38k: immutable-code mismatch and RUN-ID high-watermark

Status: partial; process smoke and exact harness gate are implemented.

## Scope

One VMHost process first accepts code A with RUN ID 1. A different same-length
code B with ID 2 is rejected with exact scope-1 `CODE_MISMATCH`. Reusing ID 2
with A is rejected with `INVALID_REQUEST_ID`, proving the rejected ID advanced
the high-watermark. B with fresh ID 3 is rejected again with `CODE_MISMATCH`,
proving the original A binding survived. Reusing ID 3 is rejected; original
code A with ID 4 then succeeds with fresh CPU state.

The process suite pins the script, exact command and source fingerprint, the
VMHost binary hash, success marker, and a 30-second outer timeout.

## Review and execution

Independent Sol/high static review passed for the smoke and harness integration.
No Python, test, build, formatting, Mini, or M1 execution was performed. The
process suite now has 37 pinned steps, all still unexecuted.

## Remaining acceptance

The process gate remains unexecuted. INVALID_STATE batch closure, broader
completion/backpressure/cleanup checks, unchanged V0 regressions, and supported
macOS ARM runtime acceptance remain open. This report makes no performance or
runtime claim.
