# Stage 3 S3-38j: V1 framing and active admission process smoke

Status: partial; the focused framing process smoke passed, while the full
39-step VMHost process suite has no completed result.

## Scope

The process smoke fragments HELLO and RUN across header/body writes, exercises
coalesced increasing RUN frames, and compares the first RUN_ACK and full RESULT
payload around the second RUN's scope-1 BUSY response. It covers EOF partway
through a frame header or body with bounded failure teardown, and an oversized
declared payload length without sending a body to pin early rejection. The
process assertion does not claim an internal staging-memory bound.

The process suite pins the script, the built VMHost input hash, exact success
marker, and a 30-second outer command timeout. The fixture uses two decoder-
supported `ADD RAX,RBX` instructions; the process checks framing behavior
without expanding the admitted instruction profile. INVALID_STATE admission
closure within a received frame batch is not covered: no deterministic process
input triggers the typed host/owner failure through the current public
protocol path.

## Review and execution

Independent Sol/high static review passed for the scenario and harness
integration. The exact command
`python3 tests/system/vmhost_control_v1_frames_smoke.py /tmp/s3-38h-run/artifacts/od-vmhost-smoke`
passed with marker `VMHost control V1 smoke passed: fragmented/coalesced
frames, bounded EOF teardown, oversized length`. The
`test_vmhost_process_manifest_covers_every_gate_once` Python self-test passed
1/1. The run covers fragmented HELLO/RUN, a second coalesced RUN rejected as
BUSY, partial-header/body EOF, and oversized-length rejection before the body.

The full `python3 tests/cpu/stage3_harness.py --suite vmhost-process` attempt
timed out at the tool call after 300 seconds with an empty captured log and no
suite summary. Its result is unconfirmed, not a pass or a test failure. The
Graphify refresh after the fixture edit timed out after 120 seconds and remains
unconfirmed. SSH to the documented Mini endpoint timed out before remote
commands or tests; M1 remains deferred by the user.

## Remaining acceptance

The focused process gate is confirmed on the current macOS ARM host. Full
39-step VMHost process acceptance remains open. INVALID_STATE batch-closure
evidence, completion races, backpressure, descriptor/lease cleanup, and
unchanged V0 regressions remain open. Performance acceptance and owner/MMU
acceptance also remain open. This result does not establish an internal
staging-memory bound, Mini/physical-x86 acceptance, or full Stage 3 completion.
