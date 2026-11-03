# Opt-in host session ABI v1

**Status:** normative design for S3-38f, 2026-11-03. This document and the
[public header](../../include/open_desktop/session_abi_v1.h) define a contract;
they do not provide exported symbols, an implementation or runtime acceptance.
The header is the sole source of C declarations and numeric constants. Swift
must import it rather than redeclare layouts. Implementations belong in
`crates/od-host/`; VMHost transport remains in `apps/vmhost/Sources/`.

This contract implements the ownership boundary selected by
[ADR-0005](../decisions/ADR-0005-persistent-same-code-session.md), with the
event-driven completion rule of
[ADR-0003](../decisions/ADR-0003-event-driven-vmhost-completion.md).
[Control protocol v1](../../protocols/control/CONTROL_PROTOCOL_V1.md) is the
sole normative wire format; its version, statuses and connection lifecycle
are distinct from this in-process ABI. MUST and MUST NOT are normative.

## Identification and compatibility

Clients MUST explicitly select this ABI and check `od_session_abi_version()`
against `OD_SESSION_ABI_VERSION_V1` and the capability bit
`OD_SESSION_CAP_S0_09_SAME_CODE_V1` from `od_session_abi_capabilities()` before
creating a session. The version is an opaque exact-match identifier, not a
wire version or an arithmetic encoding for compatibility negotiation. The
capability means the implementation supports this bounded profile; it does
not guarantee current resource availability. An unsupported host reports the
version, clears the capability and rejects create with UNSUPPORTED_HOST after
argument/code/entry validation. All other capability bits are reserved zero.
Missing symbols, a different version or a missing bit mean this opt-in path
is unavailable; the caller MUST NOT silently downgrade or replay work on v0.

[`host_abi.h`](../../include/open_desktop/host_abi.h),
[`HOST_ABI_V0.md`](HOST_ABI_V0.md), `od_host_abi_version()` and all
`od_fragment_*` contracts remain unchanged. In particular, v0 retains its
per-execute cache/arena lifetime, sequential cross-thread idle execution,
pre-run STOP semantics, status precedence and raw RFLAGS behavior. The new
types are independent of v0 even where their fields have similar meanings.

## Values, layouts and ownership

The supported layout is the macOS 64-bit C ABI: 8-bit bytes, 32/64-bit integer
widths, 64-bit pointers, native integer representation, no packed structs or
caller-selected packing. This is not a serialized format. Sizes/alignments:

| Type | Bytes | Alignment | Field offsets in bytes |
|---|---:|---:|---|
| `od_session_cpu_state_v1` | 48 | 8 | `gpr` 0, `rip` 32, `rflags` 40 |
| `od_session_create_options_v1` | 4112 | 8 | `struct_size` 0, `code_length` 4, `first_entry_rip` 8, `code` 16 |
| `od_session_run_request_v1` | 4168 | 8 | `struct_size` 0, `request_id` 4, `instruction_budget` 8, `initial_state` 16, `code_length` 64, `reserved` 68, `code` 72 |
| `od_session_run_token_v1` | 16 | 4 | `struct_size` 0, `request_id` 4, `completion_read_fd` 8, `reserved` 12 |
| `od_session_run_result_v1` | 72 | 8 | `struct_size` 0, `request_id` 4, `state` 8, `reason` 56, `reserved` 60, `instructions_executed` 64 |

The explicit size argument and input `struct_size` MUST both equal the exact
header constant. Bigger structs are not an extension mechanism. Output size
is a separate argument: output contents are not input and need no initial
`struct_size`. On success the callee writes the complete output, including
its exact `struct_size`, the invocation ID and zero reserved fields. Input
reserved fields and bytes after `code_length` in the fixed code array MUST be
zero. Length is 0 through 4096 inclusive; zero budget and empty code are valid.
Only the stated four GPRs are modeled. RIP is a code byte offset and must be
an admitted instruction start or code end. Raw RFLAGS is copied unchanged on
entry; operations model CF/PF/AF/ZF/SF/OF as in
[CPU_CONTRACT.md](CPU_CONTRACT.md). Other registers are outside this profile.

Inputs are caller-owned immutable memory for the duration of their call.
Outputs are caller-owned writable memory, exclusive to that call. Pointers
MUST be correctly aligned, live, and nonoverlapping with each other and the
proxy. Null pointers, wrong explicit sizes and recognized layout violations
return INVALID_ARGUMENT before reading a full input or writing any output.
Dangling or otherwise inaccessible nonnull pointers are caller contract
violations, not safely detectable guest errors. No input/output pointer is
retained after its call; only copied, owned values cross to the owner thread.
There are no callbacks or pointers to Rust/Swift objects in a request.

Every nonzero status leaves output bytes untouched. For create, the caller
MUST initialize its output handle slot to NULL; failure leaves it NULL. A
nonnull initial slot is INVALID_ARGUMENT and is left untouched. Success
transfers one opaque proxy handle to the caller. It cannot be dereferenced,
copied as a Rust Session, forged or reused after successful destroy. A handle
is process-local; copying its pointer does not transfer destruction authority.

## Session creation and resource lifetime

`od_session_create_v1` is synchronous. It copies immutable code and validates
S0-09 decode/admission and `first_entry_rip` before acquiring the executable
memory lease. The entry field is a preflight check for the first wire RUN,
not retained CPU state and not a request reservation. Each actual begin also
validates its own RIP; no run is executed by create.

Creation validation precedence is: pointer/size/layout/length checks;
code decode/admission (INVALID_CODE); entry boundary (INVALID_ARGUMENT);
host support (UNSUPPORTED_HOST); then required resource initialization.
The admission/preflight work MUST use the project's own decoder/contracts,
not a second Swift implementation. It may run on the calling thread using
owned value data before starting the owner; all `Session` construction remains
on its dedicated owner. No thread startup or JIT lease acquisition failure may
override invalid-code/entry precedence. Ordinary allocation/thread/descriptor
exhaustion is ALLOCATION_FAILED;
the already-held process lease is BUSY; other safely reportable typed host
state failures are INVALID_STATE. Among independent resource failures the
first actually encountered is reported, without changing semantic precedence.

Exactly one dedicated Rust OS thread creates, runs and drops `Session`, cache,
arena and published handles. A handle is returned only after that owner
reports successful initialization. Failed creation joins any partially
started owner and releases all partial resources before returning. Only the
synchronized proxy and owned requests/results/control cross threads; the
`!Send`/`!Sync` Session and its resources never do. An unsafe `Send`/`Sync`
implementation or Swift `@unchecked Sendable` cannot waive this boundary.
Any Swift wrapper must justify synchronization/lifetime of the proxy itself.

There is at most one retained session per process, including for empty code.
The existing single process-wide MAP_JIT lease stays acquired while idle.
Another session or v0 publication attempt can fail BUSY; this opt-in resource
constraint does not redefine v0's status precedence. Separate processes have
independent leases. Successful destroy releases the lease. No code replacement,
clear/reset, owner migration, shared cache, multiple sessions or resume exists.

## Begin, synchronous wait and STOP

The proxy has idle, active, completed-unconsumed, failed and destroyed states.
There is at most one invocation slot, including its completion/signaling;
there is no queue of future RUNs. `begin_run` plus `wait_run` is the synchronous
run operation split at admission so VMHost can write RUN_ACK and service STOP.
The owner is free to complete before begin returns.

`od_session_begin_run_v1` checks pointer/size/layout/length first. Under proxy
synchronization it then applies the following precedence:

1. Reject zero or an ID not greater than the proxy's high-water mark with
   INVALID_REQUEST_ID. Otherwise advance the mark, including for later rejection.
2. A failed proxy returns INVALID_STATE. Any reserved invocation or in-flight
   waiter returns BUSY, even if the owner already completed. Nothing is queued.
3. Compare length and all used code bytes with the immutable copy. A mismatch
   is CODE_MISMATCH; it never mutates the session or executes different code.
4. Validate actual entry RIP, returning INVALID_ARGUMENT on failure even for
   zero budget. Initial state is fresh; previous results do not seed it.
5. Prepare bounded owned request/result/control storage and a **fresh CLOEXEC
   pipe** for this invocation. Resource failures are ALLOCATION_FAILED or
   INVALID_STATE, with partial resources released and output untouched.
6. Atomically install its ID and new unset STOP state, then enqueue that single
   invocation to the owner. Return OK and transfer the pipe's read descriptor
   in `out_token`; its write end belongs only to the owner.

After reservation, failure to deliver/execute the request is a terminal
failure of that accepted invocation: begin still returns OK with its token,
and wait reports the typed failure. If safe publication is impossible the
helper fails fatally; it must not misreport rejection, retry or hang forever.
Before reservation an INVALID_STATE rejection makes the proxy failed; other
rejections leave its idle/current invocation unchanged except the high-water
mark. IDs never wrap; exhaustion requires a fresh helper/connection, without
automatic resubmission. No STOP advances the mark.

`od_session_request_stop_v1` may run on another host thread concurrently with
begin or wait. A null handle is INVALID_ARGUMENT. Under the same synchronization
as reservation and completion publication it applies:

- No active invocation, including completed-unconsumed/failed: NO_ACTIVE_RUN.
- Active invocation with different ID, including zero: INVALID_REQUEST_ID.
- Matching active invocation: latch STOP and return OK; repeats are idempotent.

An accepted early STOP remains set even if the owner has not entered guest
execution. STOP before admission is rejected, not saved for the next invocation.
For a validated RIP, pre-entry STOP precedes code-end and zero-budget exits;
thereafter it is observed at guest instruction boundaries. Acceptance does not
guarantee STOP_REQUESTED: natural completion may win. Completion publication
removes the active STOP target permanently; it cannot latch onto another run.

`od_session_wait_run_v1` validates output pointer/size, then checks the reserved
slot: none returns NO_ACTIVE_RUN, a different ID returns INVALID_REQUEST_ID,
and an existing waiter for that ID returns BUSY. These errors do not consume
anything. One matching waiter claims the invocation and blocks until both
execution **and notification delivery/closure** finish; no periodic polling
or replay is permitted. It may be called before notification readiness and
is the ready-result path after the notification is consumed. It never reads
or closes the caller-owned read descriptor. On successful execution/delivery
it copies the complete result and returns OK. On an ordinary terminal error
it returns status 1–6 with output untouched. Either terminal outcome consumes
the slot exactly once. A successful outcome returns the proxy to idle after
wait returns; a terminal error makes it failed and permits only STOP rejection,
wait rejection and destroy. No new begin can start while the waiter is live.

## Notification and descriptor ownership

The caller owns the returned read descriptor and MUST keep it open until wait
has returned, then close it exactly once. Create/begin failure transfers no
descriptor. The descriptor is specific to the token, not the entire session;
it MUST NOT be reused as a later invocation's notification. Consumers may use
blocking wait directly or wait on the read descriptor alongside the control
socket. Descriptor readiness alone is not an architectural result.

The owner publishes final status, owned result and request ID before writing
exactly one byte `OD_SESSION_COMPLETION_BYTE_V1`, then closes its write end.
Ordinary execution errors also receive a notification. No other writer retains
that write end. Both execution storage and signaling state stay alive until
the write/close path finishes. Normal notification is one byte followed by EOF;
EOF without the byte or read failure is a notification failure, not success.
The caller still drains via wait when safe, and closes admission. A detectable
write failure publishes INVALID_STATE, closes the write end to wake the reader
through EOF, and makes the proxy failed. A fatal owner/process failure must
terminate/wake pending control work instead of leaving an indefinite wait.
EINTR may retry notification I/O only, never guest execution.

The caller MUST NOT free the proxy or close its read end to interrupt live
execution. If read delivery fails, request cooperative STOP where still active,
await execution/signaling via wait, then tear down. No periodic worker-state
polling, reused completion flag, stale pipe byte or unbounded notification
queue substitutes for this protocol. This is a correctness contract, not a
latency or resource-performance claim.

## Status domains and protocol adapter

The header's statuses are fixed-width `uint32_t`, never Rust enum layouts or
host errno values. OK is distinct from all failures. Guest exit reasons only
occur in a successful result: CODE_END, INSTRUCTION_BUDGET, STOP_REQUESTED.
The instruction counter belongs to this invocation. Host faults, Rust panic,
invariant failure and memory corruption are not guest exits and MUST NOT be
hidden by catch-and-continue recovery or reported as budget/STOP completion.

| ABI status | Meaning / control v1 mapping |
|---|---|
| OK | Successful operation; begin maps to accepted RUN_ACK |
| INVALID_ARGUMENT | ABI argument/layout or invalid RIP; control INVALID_ARGUMENT only for a valid wire request's semantic input |
| INVALID_CODE | Create decode/admission failure; control INVALID_CODE |
| BUSY | Reserved invocation or process lease unavailable; control BUSY |
| UNSUPPORTED_HOST | Requested host profile unavailable; control UNSUPPORTED_HOST |
| ALLOCATION_FAILED | Bounded host resource initialization failed; control ALLOCATION_FAILED |
| INVALID_STATE | Safely reportable typed host/API failure; control INVALID_STATE |
| INVALID_REQUEST_ID | Nonincreasing begin ID or mismatched active/slot ID; control INVALID_REQUEST_ID where applicable |
| CODE_MISMATCH | Different immutable bytes in begin; control CODE_MISMATCH |
| NO_ACTIVE_RUN | No STOP target or no wait slot; control NO_ACTIVE_RUN for STOP |

Adapters MUST map explicitly, even though this draft's numbers correspond.
ABI misuse by VMHost (for example a wrong struct size or a mismatched wait)
is a host integration failure, not evidence that the peer sent a malformed
guest request. Raw wire structs MUST NOT be cast to these padded C layouts.

The connection retains its own high-water mark, initially zero, and applies
the normative wire ordering **before ABI calls**. It advances the mark on all
structurally valid greater RUN IDs, including BUSY/code/RIP/resource rejection;
the proxy cannot see requests rejected before it is called. The same accepted
wire ID is passed unchanged to begin, STOP and wait. The proxy's high-water
mark is an additional guard, not a replacement for the connection mark.

On the first RUN, the adapter passes that RUN's code and initial RIP to create.
It binds the connection only after begin succeeds. A failed first begin must
destroy the newly created idle/failed proxy before leaving the connection
unbound. A successful begin transfers acceptance: failure afterwards never
authorizes a second begin or code replay. On later RUNs, wire BUSY/completing
and exact-code checks precede ABI begin as required by the protocol.

After begin returns OK, VMHost installs the token/read descriptor and sends
RUN_ACK before any terminal record, even if completion is already readable.
It waits for control-socket activity and that invocation's notification
without periodic timeouts and keeps partial frame parsing incremental. On
completion it consumes the byte/EOF, calls wait for the matching ID, and emits
RESULT or a scope-2 execution ERROR. Create/begin rejections map to scope-1
ERROR; STOP maps to STOP_ACK. Wait API misuse never becomes a peer rejection.
An accepted STOP_ACK precedes that invocation's terminal record; if completion
won first, the terminal may precede the rejected STOP_ACK.

The ABI slot can become idle when wait returns, but the **connection remains
completing** until the terminal frame is written and invocation resources,
including the read descriptor, are retired. VMHost MUST reject complete RUNs
observed active/completing rather than defer them until ABI idle. One connection
uses one proxy; no reconnect/resume or automatic retry is supported.

## Destroy, failure and verification boundary

Before destroy the caller MUST prevent new calls and synchronize other proxy
callers. Destroy returns BUSY if an invocation remains reserved (including
completed but not consumed) or wait is still live. This check does not make
destroy safe against arbitrary new calls or another destroy. Null is
INVALID_ARGUMENT. Idle/failed destroy orders Session/cache/arena destruction
on the dedicated owner, joins that thread, frees the proxy and returns OK.
After OK the handle is invalid. If a safely reportable teardown failure occurs,
INVALID_STATE leaves the handle owned by the caller in failed state; live
resources cannot be freed to force progress. Fatal process failures follow
the external helper termination policy, not recoverable ABI continuation.

For EOF/protocol/transport failure VMHost closes admission first, requests STOP
for any active ID, waits for execution and signaling, closes its read descriptor,
then destroys and joins the owner. A racing NO_ACTIVE_RUN from STOP is harmless
only as an indication that completion has won; wait/drain is still required.
No terminal delivery is promised after transport loss. No failed or uncertain
RUN may be retried on a new worker or after reconnect.

This is a header/specification-only change. Static architecture review
(Astra/high) and independent final review (Sol/high) passed without outstanding
findings. Local Markdown link targets and whitespace were checked. These are
contract-only checks; no runtime checks, builds, tests, Python, Mini or M1
results are claimed. Implementation acceptance still
requires the lifecycle/failure/isolation gates in ADR-0005, including correct
publication reuse, owner-thread identity, output immutability on failure,
early/stale/racing STOP, completion/EOF ordering, descriptor/lease release and
unchanged v0 regressions. Performance choices require separate measurements.
