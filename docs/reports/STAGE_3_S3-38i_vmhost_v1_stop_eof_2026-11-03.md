# Stage 3 S3-38i: V1 STOP isolation and active-run EOF

Status: partial; process scenario and exact harness gate are implemented.

## Scope

The process smoke sends an admitted `75 FE` loop with ZF clear, waits for
RUN_ACK, sends STOP with the same request ID, then verifies STOP_ACK precedes
the terminal RESULT and that the result reports STOP. It starts a second RUN
with the same code, ZF set, and budget 1; this invocation must complete normally
without inheriting STOP. A separate VMHost process receives the loop, returns
RUN_ACK, and then sees the control socket close. The process must exit cleanly
within five seconds. The test does not require positive retirement after STOP.

The process suite pins the script, the built VMHost input hash, exact success
marker, and a 30-second outer command timeout.

## Review and execution

Independent Sol/high static review passed for the scenario and harness
integration. No test, Python, build, formatting, Mini, or M1 execution was
performed. Process termination does not independently prove descriptor or
executable-memory lease cleanup before exit.

## Remaining acceptance

The smoke is unexecuted. Broader STOP/completion races, malformed input,
backpressure, descriptor/lease cleanup and V0 regressions remain open for the
Stage 3 process acceptance suite.
