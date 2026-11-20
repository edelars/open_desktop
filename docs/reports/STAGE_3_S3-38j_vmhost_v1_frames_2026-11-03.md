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
163840 ACK bytes. A second signed test-only build kept the output limit but
disabled read throttling. The same scenario failed with a broken pipe while
writing to VMHost.

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

## Maximum-length fragmented RUN, 2026-11-19 UTC

The focused process smoke now sends a 4156-byte RUN payload with 4096 code
bytes across header and body fragments. The client withholds the final byte
and checks for premature output. Once that byte arrives, the RUN executes the
last `JNZ -2` from RIP 4094. The result must report RIP 4094, budget exhaustion
and one retired instruction. The probe covers the largest legal RUN and
crosses the 4096-byte socket read chunk. Internal staging size remains open.

The updated smoke passed ten consecutive runs against the signed VMHost from
`stage3-20261119-022002-cf975ae02b`. A scratch client fixture that sent `00`
instead of the final `FE` failed the exact RESULT comparison. The process
manifest self-test passed with the updated success marker. The previous
40/40 process run predates this fixture change.

## Active completion and staging bounds, 2026-11-19 UTC

The process test starts a long-running `JNZ -2` invocation and records the
two new pipe descriptors through macOS `proc_pidinfo(PROC_PIDLISTFDS)`. It
sends 380 nonmatching STOPs and waits until the VMHost socket is unwritable
on the duplicated host endpoint. The client then sends the matching STOP
and withholds reads until both pipe descriptors retire. The VMHost endpoint
is still unwritable at retirement. Draining yields the 380 ordered
`INVALID_REQUEST_ID` ACKs, the successful STOP ACK and the stopped RESULT
with the expected register state and RIP. The retired-instruction count is
schedule-dependent. A signed test-only build that serviced completion only
with an empty queue or a writable socket failed the pipe-retirement check.

The staging audit covers the retained decoder prefix and queued response
bytes. `ControlFrameDecoder.append` fills the 16-byte header first, validates
the declared payload against 8192 bytes, and fills only the rest of that
frame. It clears the prefix when decoding is complete. The maximum prefix
is 8208 bytes. `queueV1Frame` checks each append against the 16416-byte queue
capacity; sending only removes bytes. For ordinary reads the reactor asks
for `POLLIN` below 8224 bytes. A 4096-byte read can complete at most one
staged command plus 256 more frames. Each command queues at most 28 bytes,
and the active invocation can add one terminal frame of at most 80 bytes.
The 8192-byte reserve covers that batch and its terminal frame.

A signed diagnostic build logged `buffered.count` after each decoder append
and `outgoing.count` after each enqueue. An invalid STOP body of 8192 bytes
reached an 8208-byte prefix and failed after decoding at the command check.
Peak queue lengths were 13312 bytes in the 8192-STOP idle flood and 5120 bytes
in the active-completion scenario. These observations measure logical bytes
in the two buffers. Deterministic scope-1 `INVALID_STATE` batch closure remains
open.

The updated focused process smoke passed 30 consecutive runs. The process
manifest self-test passed with the active-completion marker.
`python3 tests/cpu/stage3_harness.py --suite vmhost-process` completed as
`vmhost_process_only` with all 40 selected steps passed and 351 unselected
steps recorded as `not_run`. Run `stage3-20261119-023609-4f275d011a` used the
changes above on base revision `b0bbaa8878d1843e788e4d5bcc8ce17358c4153c`.
The initial and final source fingerprint was
`af1452bf10657ea722e5f1e221c2ffe9fe6a5e0506fb1ca23b0524efb5e03c4c`.
The input manifest SHA-256 is
`2ff50a15ee39c50f46859be1b78bc36067643187444ea223129266cacab4b0dd`.
The summary SHA-256 is
`6731bfcbd8c218e2765af5a29e6f3b548e2d84ba4bc6ba0b8904a3a7fcf33f8f`.

## INVALID_STATE admission batch closure, 2026-11-19 UTC

The process harness builds and signs a dedicated VMHost artifact with a test
admission bridge. It injects `INVALID_STATE` during creation for a single
`ADD` and during begin for a two-`ADD` RUN. The begin case creates and
destroys the owner through the real Rust ABI. Both cases use the product V1
reactor and verify the injected call site through its fixture diagnostic.
[Test admission bridge](../../tests/fixtures/control/v1_admission_failure_bridge.h)

The client sends RUN 1 and STOP 77 together. Each failure must send exactly
the scope-1 `ERROR/INVALID_STATE` for RUN 1, then close the socket without
replying to STOP 77 and exit normally. A duplicate of the VMHost endpoint
survives until process exit to verify that STOP 77 was read from the socket.
The client checks EOF after releasing the duplicate. A diagnostic build
that read one byte per call left STOP 77 in the socket and failed that
check. Disposable Swift variants that
replaced either batch break with a continue each produced an unexpected
20-byte STOP ACK after ERROR. The focused fixture smoke passed with the
unchanged reactor and rejected both variants.

The manifest pins the alternate bridge, full Swift build command, signing and
verification steps, and exact smoke marker. The process summary binds that
marker to the signed artifact hash. The normal smoke and app-bundle use the
standard bridge. The injected statuses verify reactor behavior on typed
ABI rejection. Physical x86 and Mac mini checks remain deferred.

The strengthened fixture smoke passed 30 consecutive runs. Eleven process
harness self-tests passed. In `stage3-20261119-024905-ba581bf81f`,
`python3 tests/cpu/stage3_harness.py --suite vmhost-process` completed as
`vmhost_process_only` with 46 steps passed and 351 unselected steps at
`not_run`. The admission-failures, normal framing and generated-fault gates
passed. The run applied these changes to base revision
`267f83da68e4d4276547b5f28c2f7fd958069cbb`. Its initial and final source
fingerprint was `3d2b002c2210310a8ae3e552e93d88255f50a009ad70e62af8fd79ac3898598a`.
The signed admission-failure artifact SHA-256 is
`28ee616e2188d54a08e1c398641c94391688b88a7d4b2930435769ddd2b660b5`.
The input manifest SHA-256 is
`13b4889a9a928d692253f9c213fc5cd85b7e1793329a0c85e07ff3ed964a0722`.
The summary SHA-256 is
`88da852c655cc4c27d74df46d845acc6bdfa4b3b9897fec267f2c9841d6bdd74`.
