# ADR-0005: Opt-in persistent same-code VMHost session

- **Date:** 2026-11-03.
- **Status:** accepted design boundary for the proposed S3-38f slice; not implemented or runtime-validated.
- **Related documents:** [architecture](../ARCHITECTURE.md#6-граница-swift--rust), [project layout](../PROJECT_LAYOUT.md), [Stage 3 plan](../plans/STAGE_3.md), [S3-38e session owner](../reports/STAGE_3_S3-38e_session_owner_2026-11-03.md), [completion ADR-0003](ADR-0003-event-driven-vmhost-completion.md), [control v0](../../protocols/control/CONTROL_PROTOCOL_V0.md), [host ABI v0](../specs/HOST_ABI_V0.md).

## Question

How can a VMHost retain the bounded S0-09 session across sequential RUN requests
without changing existing v0 lifecycle guarantees or moving thread-affine JIT
ownership through Swift?

## Context

S3-38e introduced an opt-in Rust `od-host::session::Session`. It retains copied,
immutable code, admission, frozen fixture identity/dependencies, a bounded
cache and an executable-memory backend. Each call borrows fresh CPU state,
budget and STOP. Its `Rc`-based published handles make it `!Send` and `!Sync`;
creation, execution, clear and drop belong to one owning thread. The current
process-wide MAP_JIT lease admits only one executable-memory owner at a time.
The S3-38e report records added correctness gates as unexecuted, not acceptance
evidence for IPC integration.

Control v0 explicitly serves one RUN per connection and defines no persistent
session. The public v0 fragment ABI permits sequential execution of an idle
handle from different host threads; its cache and arena are local to each
execute call and released before that call returns. Inserting a retained
`Session` into that handle would change both ownership and lease lifetime.

ADR-0003 requires event-driven completion while servicing STOP and calls for
reconsideration when the lifecycle becomes persistent. Its one-fragment
measurements do not establish persistent-session latency or resource budgets.
This ADR performs that architectural reconsideration without replacing its
event-driven completion rule or claiming a performance improvement.

## Options

1. Silently accept repeated RUN on control v0 and retain a session in the v0
   fragment handle. Rejected: this changes documented protocol and ABI behavior.
2. Put the session behind Swift `@unchecked Sendable` and reuse dispatched
   worker closures. Rejected: synchronization or that annotation alone does
   not establish a single owning OS thread for thread-affine Rust objects.
3. Add an explicitly selected control v1 path and a separate session ABI proxy
   whose dedicated Rust thread owns the session. Selected for the bounded
   ownership contract below, subject to implementation verification.

## Decision

### Boundary and scope

S3-38f will use an explicitly opt-in, versioned control v1 path over the local
inherited socketpair. Default v0 behavior stays unchanged; peers must not infer
v1 from a second v0 RUN or silently downgrade/replay a v1 operation. A separate
session ABI exposes an opaque, synchronized control proxy. Its new symbols and
version/capability identification must be documented before implementation;
`od_host_abi_version()` and the existing v0 ABI remain unchanged.

This ADR is a design decision, not a normative v1 wire format or a declaration
that v1 exists. The future normative specification belongs in
`protocols/control/`; public C declarations belong in `include/open_desktop/`.
Frame encoding, numeric values, payload layout and exact error codes must be
specified there before implementing the transport or ABI.

One VMHost serves one connection with at most one retained session and exactly
one immutable code identity once a session is created. The first accepted RUN
creates the session from validated, copied S0-09 code of at most 4096 bytes.
Subsequent RUN requests must identify the same byte sequence; using the full
code bytes and checking exact equality is sufficient for this bounded slice.
Different code is rejected without executing it or replacing the session.

Each RUN supplies fresh CPU state, instruction budget and invocation-local STOP
state. Results do not implicitly seed the next RUN. Only code, admission,
frozen fixture identity, cache and arena persist. There is no code replacement,
session reset/clear command, reconnect/resume, overlapping execution or
multi-session scheduling in this slice.

### Owner thread and ABI proxy

The proxy starts one dedicated Rust OS thread. That thread creates, runs and
drops `Session`, its cache and executable-memory backend. Creation returns a
handle only after the owner reports successful initialization; failure leaves
the output handle null and cleans up the partially created worker/resources.
Request and response storage is bounded: one active invocation and no queue of
future RUN operations.

Only owned request values and synchronized result/control state cross the
thread boundary. No `Session`, arena, cached published handle or pointer to
those thread-affine objects crosses into Swift or another execution thread.
No `unsafe impl Send/Sync` is added to bypass their ownership constraints.
Swift may share only the documented thread-safe proxy and value data; any
`@unchecked Sendable` wrapper requires an explicit synchronization/lifetime
argument for that proxy, not for the Rust session.

The session execute operation is synchronous from its caller's perspective,
while the dedicated thread performs execution. Concurrent execute on the same
proxy returns BUSY. A STOP operation may run concurrently and touches only
synchronized invocation control. No caller-owned input/output pointer is
retained beyond its ABI call. Nonzero execution status leaves the result
output untouched; host/API failure is distinct from a guest exit reason.

### Request admission, STOP and completion

The connection has unbound, idle, active, completing and closing states.
Request IDs for RUN are nonzero and strictly increasing without wraparound;
the receiver retains a high-water mark, including well-formed rejected RUN
IDs, rather than an unbounded history. Exhaustion requires a new helper and
connection. STOP refers to the active RUN ID, not to a new RUN ID.

Only an idle or unbound connection may admit a RUN. Validation, reservation of
its ID and publication of its fresh control state precede its successful ACK.
The owner may then execute; even if execution finishes immediately, the
control writer must send the successful RUN_ACK before its terminal record.
An early STOP after admission must be retained if the owner has not entered
execution. No operation silently queues a future RUN. Pipelined RUN received
before the active request's terminal record is sent is rejected; it must not
be retained and admitted later merely because the previous run finished.
This is receiver-local ordering: a complete RUN decoded or observed in a
buffer while active/completing cannot be deferred across the transition to
idle. It does not require proving when the peer sent bytes not yet received.

STOP can affect only the matching active invocation. Admission, STOP acceptance
and completion publication must be serialized so a STOP racing completion
cannot latch onto the next run. Idle, stale and wrong-ID STOP are rejected
without changing session state. Successful STOP_ACK acknowledges acceptance,
not a particular guest exit: natural completion may win, and the terminal
record remains final. Once completing, the invocation's STOP storage is no
longer available for new requests and is discarded after cleanup.

Implementation must follow **ADR-0003**: the control loop waits on the control
socket and completion descriptor without periodic worker-state polling, and
remains able to process STOP while execution is active. Allocate a **fresh
completion notification/pipe and completion record for each RUN**. The owner
thread persists, but a notification, semaphore count or `done` flag from a
previous RUN must not be reused as the next invocation's completion signal.

For every admitted RUN, publish its final status/result and request identity
before writing its one completion notification. Keep invocation storage and
both pipe endpoints alive until signaling has finished. The control side
consumes that invocation's notification and emits one terminal RESULT or
execution ERROR while transport is healthy; a rejected command's error is
not the active RUN's terminal record. A next RUN is admitted only after the
previous terminal record has been written and its signaling/resources have
been retired. Framing must remain incremental so a partial control frame
cannot block observation of worker completion.

Interrupted notification I/O may retry the system call, never guest execution.
Broken notification delivery must wake or terminate the control path through
an explicit failure/EOF mechanism; it must not leave an indefinite wait for a
result that can no longer be signaled. No periodic polling fallback is added.

### EOF, destroy and failures

EOF, framing/version failure or transport failure closes admission first.
Request cooperative STOP for any active invocation, await execution and
notification completion, then destroy the session on its owner thread and
join that thread before releasing the proxy/descriptors. No RESULT delivery
is promised after transport failure. The existing external helper termination
policy remains the escape from an unresponsive or fatally failed process;
cleanup must not free live JIT state to force an in-process timeout.

The session ABI destroy operation returns BUSY while execute is active. Before
destroy, its caller must exclude new calls and synchronize other in-flight
proxy operations. A successful destroy joins the owner after session/cache/
arena destruction and releases all proxy resources. A destroyed handle cannot
be reused. IPC teardown therefore waits for active execute before destroy.

Rust panic, worker invariant failure and generated-code memory faults remain
host failures. They are not translated into guest STOP/BUDGET or hidden by
catch-and-continue recovery. A fatal worker failure closes/terminates the
helper and wakes pending control work. No failed or uncertain RUN is retried,
replayed on a replacement worker, or automatically rerun after reconnect.

### Compatibility and executable-memory lease

Existing `od_fragment_*` symbols, ABI id, structs, statuses, output ownership,
raw RFLAGS, reserved-zero result field, pre-run STOP behavior and per-call
cache/arena lifetime remain unchanged. Sequential idle v0 execution across
different host threads and independence of v0 handles remain regression gates.
Existing v0 status precedence is preserved, including BUSY when another
process-local mapping is live and invalid-entry handling before STOP/budget.
The new session ABI must state its own admission/status precedence explicitly;
the thread-affine Rust API is not automatically a v0-compatible adapter.

The process-wide single MAP_JIT lease is unchanged. An idle persistent session
continues to own it; a second owner or v0 publication attempt receives the
existing busy/mapping-already-live failure. This is an explicit opt-in resource
constraint, not a promise that legacy execution succeeds beside a retained
session. Destruction releases the lease. Two separate VMHost processes retain
independent sessions/resources; simultaneous sessions in one process are not
introduced. Invocation state and STOP never leak between handles or processes.

## Implementation and acceptance obligations

VMHost transport and lifecycle stay in `apps/vmhost/Sources/`; the session proxy
and host/JIT resource boundary stay in `crates/od-host/src/`. This needs only
the existing `od-host` dependencies on `od-jit`/`od-isa` and standard host
synchronization. It introduces no `od-machine` or Stage 2 work.

Before claiming implementation acceptance, verify:

- Repeated same-code RUN with fresh state/budget/STOP returns correct complete
  results and demonstrably reuses a publication; output equality alone does
  not prove cache reuse.
- Owner creation/run/drop occur on one OS thread; failed creation and idle/
  active teardown release worker, descriptors and the executable-memory lease.
- Immediate, delayed, stale and wrong-ID STOP; natural-completion races;
  rejected pipelining; increasing/exhausted IDs; different code; malformed,
  chunked and partial frames; EOF idle/running; notification/worker failure.
- No stale completion or STOP affects the next RUN; status failure leaves
  output untouched; a new owner can acquire the lease after successful destroy.
- Existing v0 lifecycle/status/output/cross-thread/two-handle checks and
  two-process isolation remain valid alongside opt-in v1 coverage.

Measure repeated cold/warm and STOP/completion latency and retained resources
under comparable representative conditions before selecting performance
policy or claiming acceleration. This design supplies no such measurements,
does not replace default v0, and does not close S3-38 or Stage 3.

## Deferred decisions and verification status

Multiple sessions or immutable programs in one connection/process, code
replacement, shared caches, owner-thread migration, overlapping RUN,
architectural-state continuation, active-code retirement, memory execution,
owner-MMU, SMC/DMA coherence and remote/reconnecting sessions require separate
scope and architecture review. Changes to the accepted peer-visible lifecycle
or wire semantics require a corresponding protocol decision/version; a new
MAP_JIT ownership policy requires a separate resource-lifetime decision and
measurements. None is implied by this bounded session.

This change adds only this ADR. No normative v1 protocol, C header or runtime
code is added, and no tests, builds, Mini or M1 checks were run for this design.
Runtime risks listed above are acceptance obligations, not newly confirmed
implementation defects.
