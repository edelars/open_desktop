# Stage 3 S3-38k: immutable-code mismatch and RUN-ID high-watermark

Status: focused process acceptance passed; broader V1 acceptance remains open.

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

Independent Sol/high review passed after correcting the supported instruction
fixtures and RAX expectations. The focused process smoke passed on the current
ARM64 host against the previously built, signed VMHost binary, with the exact
success marker. Both focused harness manifest tests passed (2/2). The full
process suite contains 39 steps and remains unverified; an earlier suite
invocation timed out at the tool boundary without a summary, so its result is
unknown.

An SSH availability check for the Mini (`senya@192.168.1.14:22`) timed out
before any remote command ran. M1 regression remains deferred. No build or
formatting check was needed for this fixture-only change.

## Remaining acceptance

INVALID_STATE batch closure, broader completion/backpressure/cleanup checks,
unchanged V0 regressions, full process-suite execution, and physical Mini
acceptance remain open. This report makes no performance claim.
