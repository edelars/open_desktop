# S3-38g: VMHost control V1 transport implementation status

**Date:** 2026-11-03. **Status:** implemented, statically reviewed, and unexecuted;
partial S3-38 slice.

## Scope

The Swift VMHost accepts control V1 only when the first HELLO frame explicitly
selects version 1. The V1 path uses the public
[`session_abi_v1.h`](../../include/open_desktop/session_abi_v1.h) declarations
to create at most one S0-09 same-code session, process sequential increasing
RUN IDs, handle exact-ID STOP, and return per-invocation RESULT or scoped ERROR.
One control thread waits on the control socket and each run's fresh completion
descriptor. Frame input is incremental and stages no more than one maximum-size
frame; output is nonblocking and capped at 16,416 queued bytes. The VMHost
bridging header imports both public ABI headers.

The wire contract remains
[`CONTROL_PROTOCOL_V1.md`](../../protocols/control/CONTROL_PROTOCOL_V1.md).
No Rust crate, public ABI header, or normative protocol document was changed in
this slice.

## V0 compatibility boundary

The initial frame version selects the path. Control V0 continues through its
existing one-RUN fragment lifecycle, V0 frame version, and existing
`od_fragment_*` calls. V1 is never inferred from repeated V0 RUNs and does not
change V0 handle ownership or execution lifecycle. Runtime V0 regressions have
not been run for this change.

## Review and execution

Independent full Sol/high static review passed after fixes for successful STOP
status mapping, bounded nonblocking output, closing admission on RUN
`INVALID_STATE`, retiring a consumed invocation before notification-failure
teardown, and limiting decoder staging to one frame.

No Swift build, test, signing, Python, Mini, M1, or runtime execution was
performed. No wire-level runtime or performance claim is made.

## Remaining acceptance

S3-38g remains partial until runtime checks cover repeated same-code RUN with
fresh state and budget, exact code mismatch and ID high-water behavior,
active/completing RUN rejection, early/wrong/stale/racing STOP, ACK and terminal
ordering, chunked and malformed frames, EOF while idle and running,
backpressure, notification failure, session-owner teardown and descriptor/
executable-memory lease release. The existing V0 lifecycle and process
regressions must also pass on a supported macOS ARM host. These checks are
deferred; the S3-38 and Stage 3 acceptance gates remain open.
