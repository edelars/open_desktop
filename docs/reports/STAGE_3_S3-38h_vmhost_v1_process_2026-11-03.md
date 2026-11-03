# Stage 3 S3-38h: VMHost Control V1 repeated RUN process smoke

Status: partial; source and fail-closed process-harness gate are implemented.

## Scope

`tests/system/vmhost_control_v1_same_code_smoke.py` launches one built VMHost
and sends two sequential RUN frames over one local socket pair. Both invocations
use the same immutable two-instruction fragment but different CPU snapshots,
budgets, and increasing request IDs. The smoke checks READY, exact RUN_ACK and
full RESULT payloads in order, then closes the control connection while idle
and requires a successful VMHost exit.

The Stage3 process suite pins the command, the smoke and ABI source paths, the
VMHost binary inputs, timeout, and success marker. Its Swift compilation uses
the VMHost bridging header that imports both public host and session ABI
headers.

## Review and execution

Independent Sol/high static review passed. No Python execution, build, test,
formatting, Mini, or M1 run was performed. The process gate and harness
self-check are pinned but unexecuted.

## Remaining acceptance

The smoke currently covers repeated same-code RUN with fresh state/budget,
monotonic IDs, ACK/RESULT ordering, and idle EOF teardown. STOP during RUN, EOF
during RUN, malformed/partial frames, completion races, backpressure, descriptor
and lease teardown, and unchanged V0 process regressions still require runtime
acceptance on a supported macOS ARM host. This report makes no runtime or
performance claim.
