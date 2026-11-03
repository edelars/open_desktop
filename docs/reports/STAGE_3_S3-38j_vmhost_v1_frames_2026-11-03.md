# Stage 3 S3-38j: V1 framing and active admission process smoke

Status: partial; process scenario and exact harness gate are implemented.

## Scope

The process smoke fragments HELLO and RUN across header/body writes, exercises
coalesced increasing RUN frames, and compares the first RUN_ACK and full RESULT
payload around the second RUN's scope-1 BUSY response. It covers EOF partway
through a frame header or body with bounded failure teardown, and an oversized
declared payload length without sending a body to pin early rejection. The
process assertion does not claim an internal staging-memory bound.

The process suite pins the script, the built VMHost input hash, exact success
marker, and a 30-second outer command timeout. INVALID_STATE admission closure
within a received frame batch is not covered: the author found no deterministic
process input that triggers the typed host/owner failure through the current
public protocol path.

## Review and execution

Independent Sol/high static review passed for the scenario and harness
integration. No test, Python, build, formatting, Mini, or M1 execution was
performed. Socket read boundaries and runtime teardown remain unverified.

## Remaining acceptance

Run these process gates on the supported macOS ARM host at the deferred Stage 3
acceptance point. INVALID_STATE batch-closure evidence, completion races,
backpressure, descriptor/lease cleanup, and unchanged V0 regressions remain
open.
