# Stage 3 S3-38i: V1 STOP isolation and active-run EOF

Status: partial; the bounded process smoke passed, while wider process and
Stage 3 acceptance remain open.

## Scope

The process smoke sends an admitted `75 FE` loop with ZF clear, waits for
RUN_ACK, sends STOP with the same request ID, then verifies STOP_ACK precedes
the terminal RESULT and that the result reports STOP. It starts a second RUN
with the same code, ZF set, and budget 1; this invocation must complete normally
without inheriting STOP. In a separate process case, the harness sends the
loop, waits for RUN_ACK, and closes the control socket. VMHost must exit cleanly
within five seconds. The test does not require positive retirement after STOP.

The process suite pins the script, the built VMHost input hash, exact success
marker, and a 30-second outer command timeout.

## Review and execution

Independent Sol/high static review passed for the scenario and harness
integration. The exact command
`python3 tests/system/vmhost_control_v1_stop_eof_smoke.py /tmp/s3-38h-run/artifacts/od-vmhost-smoke`
passed with the marker `VMHost control V1 smoke passed: STOP isolation and
bounded active-run EOF teardown`. It verified STOP_ACK before terminal RESULT,
the stopped result and unchanged CPU state, a fresh same-code invocation that
completed normally with ZF set and budget 1, and clean exit of a separate
process after active-run EOF. The shared process-manifest self-test passed in
the S3-38h acceptance slice.

Process termination does not independently prove descriptor or executable-
memory lease cleanup before exit. No completed full-suite result is available.
No Mini or physical-x86 test ran; the Mini SSH endpoint timed out before remote
tests. M1 remains deferred by the user.

## Remaining acceptance

Broader STOP/completion races, malformed input, backpressure, descriptor/lease
cleanup and V0 regressions remain open for the Stage 3 process acceptance
suite. Full Stage 3 acceptance remains open.
