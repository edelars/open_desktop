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

## Signed VMHost socket backpressure, 2026-11-19 UTC

On the local macOS ARM host, a traced build entered `send(MSG_DONTWAIT)` on
a blocking AF_UNIX socket before the client resumed reading and returned only
after reads resumed. V1 now enables `O_NONBLOCK` after sending READY, before
entering the reactor. The traced build with the fix returned `EAGAIN` while
the client withheld reads, leaving the reactor free to poll completion.

The process gate checks the VMHost end's descriptor flags after the first RUN
has completed. It then withholds reads while sending 8192 STOP frames and
requires blocked client writes with ACKs pending. After resuming reads, it
compares the entire stream of `NO_ACTIVE_RUN` ACKs, completes a second RUN and
checks clean EOF teardown. The new gate rejected the signed baseline binary at
`1204eb52305285a55ba233e81a5358c68c262408` because its V1 descriptor remained
blocking. It passed on the signed traced build with the fix. The process
manifest self-test passed 1/1 with the updated marker.

The traced build passed 20 consecutive runs. A signed test-only build that
dropped one queued byte on each `EAGAIN` failed the recovery check after
sending all 131072 request bytes and receiving 163799 of the expected
163840 ACK bytes.

`python3 tests/cpu/stage3_harness.py --suite vmhost-process` completed as
`vmhost_process_only` with all 40 selected steps passed, including
`process_control_v1_frames` and `process_generated_fault`. The 351 unselected
steps retained `not_run`. Run `stage3-20261119-022002-cf975ae02b` used
base revision `1204eb52305285a55ba233e81a5358c68c262408` with the S3-38j
changes and identical initial and final source fingerprints
`35dfffa4959ea8b4150fe7d08ef3cc5f03071ccb7b353cae8d2c70dbc8e8820e`.
The input manifest SHA-256 is
`b894238551f874351b27aa3886886388a2bd42b0fcc559ca881eff337dd1053e`.
The summary SHA-256 is
`b0f9882b60550c9b0ed58741eaf654325a5f73e4ac6a84509c6940b5c0c4fb1b`.

Deterministic INVALID_STATE batch closure and an internal staging-memory
bound still require separate evidence. The pressure phase starts in idle
state. Completion of an active RUN while the socket is full needs separate
process evidence. Physical x86 and Mac mini checks remain deferred.
