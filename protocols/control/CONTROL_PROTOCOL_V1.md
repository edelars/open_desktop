# Control Protocol v1: bounded same-code session

**Status:** normative design for the opt-in S3-38f path; implementation and
runtime acceptance are not claimed. This specification follows
[ADR-0005](../../docs/decisions/ADR-0005-persistent-same-code-session.md) and
[ADR-0003](../../docs/decisions/ADR-0003-event-driven-vmhost-completion.md).
The words MUST, MUST NOT and MAY express protocol requirements.

## Scope and transport

The transport is one connected local Unix stream `socketpair` between a
launcher and its child VMHost. The launcher passes the helper endpoint as file
descriptor 3. One helper serves one connection and at most one immutable S0-09
session, with sequential RUN operations over exactly the same code bytes.
There is no listener, reconnect, multiplexing, remote transport or authentication
layer. Possession of the inherited endpoint is the local transport boundary.

Both peers MUST explicitly select control v1. A frame header version of `1`
selects this protocol; it is not a C ABI version. No fallback, downgrade,
replay or retry of an uncertain RUN is permitted. Control v0 remains separately
defined by [CONTROL_PROTOCOL_V0.md](CONTROL_PROTOCOL_V0.md), including its one
RUN per connection limitation. A second v0 RUN MUST NOT enable this protocol.

## Encoding and frame

All integer fields are unsigned little-endian. There is no padding or implicit
alignment on the wire. Frames consist of the following 16-byte header followed
by exactly `payload_length` bytes:

| Offset | Bytes | Field | Required value |
|---:|---:|---|---|
| 0 | 4 | Magic | ASCII `ODCP` (`4f 44 43 50`) |
| 4 | 2 | Protocol version | `1` |
| 6 | 2 | Kind | Value in the message table |
| 8 | 4 | Request ID | `0` only for HELLO/READY; otherwise nonzero |
| 12 | 4 | Payload length | At most `8192` |
| 16 | payload length | Payload | Exact format for the kind |

Maximum total frame length is `8208` bytes. Readers MUST validate the header
and length before reserving payload storage and MUST handle partial reads and
writes; a system call does not define a frame boundary. Magic, version,
direction, kind, fixed payload length, reserved fields and RUN length
inconsistencies are fatal protocol errors. Unknown values are not extensions.
Fatal framing/protocol errors close the connection without an ERROR response;
the active invocation, if any, follows the teardown rules below.

## Messages and handshake

| Kind | Name | Direction | Payload bytes | Meaning |
|---:|---|---|---:|---|
| 1 | HELLO | Launcher → VMHost | 0 | First frame, request ID 0 |
| 2 | READY | VMHost → Launcher | 8 | Successful handshake, request ID 0 |
| 3 | RUN | Launcher → VMHost | 60 + code length | Execute fresh invocation state |
| 4 | RUN_ACK | VMHost → Launcher | 4 | Accepted RUN; status MUST be 0 |
| 5 | STOP | Launcher → VMHost | 0 | Stop matching active invocation |
| 6 | STOP_ACK | VMHost → Launcher | 4 | STOP status, including rejection |
| 7 | RESULT | VMHost → Launcher | 64 | Successful accepted RUN completion |
| 255 | ERROR | VMHost → Launcher | 12 | Rejected RUN or accepted RUN failure |

READY contains `u32 session_profile = 1` at offset 0 and `u32 reserved = 0`
at offset 4. Profile 1 means exactly this document's bounded S0-09 same-code
session; it is neither the host ABI version nor an extensible capability mask.
Any other profile or reserved value is a protocol error. The separate session
C ABI declarations/version contract must be specified before its implementation;
they are not negotiated by this wire protocol.

The launcher MUST send HELLO and wait for READY before sending any RUN. The
helper sends READY only once. Subsequent HELLO, handshake messages in the wrong
state, zero IDs on other messages, and messages in the wrong direction are
fatal protocol errors. READY indicates protocol/profile support, not acquisition
of a JIT mapping or successful session creation.

## Status and ERROR payloads

RUN_ACK and STOP_ACK contain one `u32 status` at offset 0. The status namespace
below belongs to control v1. Values 0–6 preserve the familiar v0 status meanings;
an adapter MUST map errors explicitly rather than expose arbitrary Rust enums.

| Value | Name | Meaning |
|---:|---|---|
| 0 | OK | Request accepted |
| 1 | INVALID_ARGUMENT | RUN has an invalid entry RIP or invocation argument |
| 2 | INVALID_CODE | First RUN code fails S0-09 decode/admission |
| 3 | BUSY | RUN arrived during active/completing state, or session mapping acquisition is busy |
| 4 | UNSUPPORTED_HOST | Host cannot provide the requested execution profile |
| 5 | ALLOCATION_FAILED | Required bounded host resource could not be allocated |
| 6 | INVALID_STATE | Typed host/API state failure; not a guest exception |
| 7 | INVALID_REQUEST_ID | RUN ID is not greater than the high-water mark, or active STOP ID does not match |
| 8 | CODE_MISMATCH | RUN code differs from the session's immutable bytes |
| 9 | NO_ACTIVE_RUN | STOP has no active invocation to affect |

ERROR has the following exact payload:

| Offset | Bytes | Field | Values |
|---:|---:|---|---|
| 0 | 4 | Status | Nonzero status from the table |
| 4 | 4 | Scope | `1` = rejected command; `2` = accepted RUN execution failure |
| 8 | 4 | Command kind | `3` (RUN), the only command using ERROR in v1 |

Scope 1 is a rejection and MUST NOT complete or cancel an already accepted
RUN, even when the rejected command illegally reused that RUN's ID. Scope 2
is the terminal record for an accepted RUN and MUST use that active ID; only
statuses 1–6 are permitted in this scope. STOP rejections use STOP_ACK, not
ERROR. Unknown scope/status/kind combinations or ERROR with status OK are
protocol errors. This scope field is mandatory: kind and request ID alone do
not distinguish a duplicate-RUN rejection from execution failure.

## RUN input and immutable session

The payload is a 60-byte prefix followed by the code:

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 4 | Code length, from 0 through 4096 |
| 4 | 8 | Instruction budget |
| 12 | 8 | Initial RAX |
| 20 | 8 | Initial RCX |
| 28 | 8 | Initial RDX |
| 36 | 8 | Initial RBX |
| 44 | 8 | Initial RIP, byte offset into code |
| 52 | 8 | Initial raw RFLAGS |
| 60 | code length | Immutable x86 instruction bytes |

Payload length MUST equal `60 + code_length`; the largest RUN payload is
`4156` bytes. A short prefix, oversized code length or inconsistent total
length is a fatal protocol error, not a RUN rejection. Zero code length and
zero instruction budget are valid. CPU forms and valid entry/branch boundaries
are defined by [CPU_CONTRACT.md](../../docs/specs/CPU_CONTRACT.md). RFLAGS is
copied without normalization; CF/PF/AF/ZF/SF/OF are the modeled arithmetic flags.
Only the four listed GPRs are inputs/outputs of this profile.

The first accepted RUN validates and copies code, creates the retained session,
and binds the connection to those exact bytes. A rejected first RUN does not
bind code or retain a partially created session. Every subsequent RUN carries
the full code and MUST compare equal byte-for-byte, including its length.
Hashes alone are insufficient. Different code receives CODE_MISMATCH, without
replacement, execution or mutation of the existing session.

Each accepted RUN has fresh CPU state, budget, STOP state and result storage.
Previous RESULT state is never an implicit next input. No invocation pointer
or STOP value is retained as part of a cached artifact. Code/admission/cache/
arena may persist until connection teardown; no wire command clears them.

## Request IDs, admission and rejection

The helper keeps a RUN ID high-water mark, initially 0. A structurally valid
RUN with ID greater than the mark advances it before semantic admission,
including when that RUN is subsequently rejected. IDs less than or equal to
the mark receive scope-1 ERROR/INVALID_REQUEST_ID and do not change it. IDs
cannot wrap; after ID `0xffffffff` a new RUN requires a fresh helper/connection,
with no automatic resubmission. STOP does not consume or advance RUN IDs.

For a structurally valid RUN, checks have this order:

1. Check/advance the ID high-water mark as above.
2. If active or completing, reject with scope-1 ERROR/BUSY. Do not enqueue it.
3. If bound, compare immutable code; reject mismatch with scope-1 ERROR/CODE_MISMATCH.
4. If unbound, validate code; reject failure with scope-1 ERROR/INVALID_CODE.
5. Validate RIP: it MUST be an admitted instruction start or `code_length`.
   Reject failure with scope-1 ERROR/INVALID_ARGUMENT, even with STOP or zero budget.
6. Prepare bounded invocation resources and, if unbound, the owner/session.
   Reject ordinary resource/host failure with scope-1 ERROR/status 3–6 and
   release partial resources. INVALID_STATE closes the connection after ERROR;
   other rejections leave it unbound/idle as applicable.
7. Atomically install the active ID and fresh STOP/control state, then dispatch
   this invocation to the owner. Send RUN_ACK/OK before any terminal response.

A successful RUN_ACK means accepted, not completed. The owner may finish before
the ACK write completes, but the control writer MUST preserve ACK-before-terminal
order. Failure to submit an already admitted invocation is a scope-2 terminal
host failure; it is not converted to a rejection or retried.

The launcher MUST wait for the terminal record before sending its next RUN.
At most one RUN is admitted; there is no future-RUN queue. A complete RUN
decoded or observed buffered while active/completing MUST be rejected in that
state and cannot be deferred across the next idle transition. This is local
receive ordering, not a requirement to infer when unread bytes were sent by
the peer. Rejected pipelined RUNs still consume increasing IDs as above.

## STOP and terminal records

After RUN_ACK, the launcher MAY send STOP using the accepted RUN ID. The helper
serializes STOP acceptance against admission and completion publication:

- With no active invocation (including completing), return STOP_ACK/NO_ACTIVE_RUN.
- With a different active ID, return STOP_ACK/INVALID_REQUEST_ID.
- With a matching active ID, latch STOP and return STOP_ACK/OK. Repeated matching
  STOP while still active is idempotent and returns OK. It never resets STOP.

Each structurally valid STOP receives one STOP_ACK while transport remains
healthy. An accepted STOP is retained if the owner has not entered execution
and is observed at a guest instruction boundary. It is invocation-local and
cannot affect a subsequent RUN. No STOP before RUN admission is latched.
STOP_ACK/OK acknowledges acceptance, not a guaranteed STOP_REQUESTED exit;
natural completion can win the execution race. An already published completion
is final and is not modified by later STOP.

When a STOP was accepted before completion publication, the control writer
MUST send its STOP_ACK before the active RUN's terminal record. If completion
won first, RESULT/terminal ERROR may precede the rejected STOP's STOP_ACK.
The launcher MUST treat the terminal record as final and must not reinterpret
its result as failure because a racing STOP was rejected. Subsequent STOP_ACKs
are correlated by their ID and are not completions of a new RUN.

For each accepted RUN, while transport and helper remain healthy, exactly one
terminal record follows RUN_ACK: RESULT on success or ERROR with scope 2 on an
ordinary execution failure. Rejections and STOP_ACKs are never terminal for
that RUN. Scope-2 ERROR closes the connection after its frame is written; no
new RUN is admitted because continuation after execution failure is outside
this slice. Fatal process failure or transport loss may prevent a terminal
record entirely; its absence never authorizes replay.

RESULT contains exactly 64 bytes:

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 8 | RAX |
| 8 | 8 | RCX |
| 16 | 8 | RDX |
| 24 | 8 | RBX |
| 32 | 8 | RIP, byte offset |
| 40 | 8 | Raw RFLAGS |
| 48 | 4 | Exit reason: `0` code end, `1` instruction budget, `2` cooperative STOP |
| 52 | 4 | Reserved, MUST be zero |
| 56 | 8 | Retired guest instructions for this invocation |

No other exit reason is defined. Host/API errors do not produce RESULT or a
partial architectural result. For admitted valid RIP, pre-entry STOP takes
precedence over natural code-end or zero-budget completion, including RIP at
`code_length`; otherwise normal S0-09 execution determines the exit.

## Completion and incremental I/O requirements

The logical lifecycle is unbound → active → completing → idle, with first
successful admission binding the immutable code. Any fatal error/EOF enters
closing. Active ends when the owner publishes completion under the same
synchronization used for STOP acceptance. Completing ends only after the
terminal frame is written and invocation signaling/resources are retired.

The owner creates, executes and destroys the retained Rust Session on one
dedicated OS thread behind a synchronized proxy. Thread-affine `!Send`/`!Sync`
Session, arena and published handles MUST NOT cross into Swift or other
execution threads. The process-wide single MAP_JIT lease is unchanged: an
idle retained session still owns it; another process-local publication attempt
can fail BUSY. A second session is not exposed by this protocol. Separate
VMHost processes remain independent.

Per ADR-0003/ADR-0005, each admitted RUN MUST have a fresh completion record
and fresh notification pipe. The owner publishes result/status and ID before
writing that invocation's notification. A previous pipe byte, semaphore count
or done flag MUST NOT be reused for the next RUN. Both endpoints and storage
remain live through notification completion; retirement precedes next admission.

The control loop MUST wait for both control-socket activity and completion
without periodic worker polling. Frame parsing MUST be incremental: a partial
header/payload is retained within the maximum frame bound while the loop can
still observe completion. It MUST NOT block trying to finish a partial STOP
or RUN before delivering an already completed invocation's terminal record.
If a full RUN was observed while active/completing, the rejection rule still
applies; partial bytes alone do not imply an admitted or queued request.

Input staging is bounded by one maximum frame; output staging MUST be bounded
by two maximum frames (`16416` bytes), with backpressure rather than an
unbounded response queue. Pending ACK ordering and capacity for the active
terminal record MUST be maintained. Partial writes require readiness handling
and MUST NOT prevent the owner from completing. Notification I/O interrupted
by EINTR may retry that I/O, never guest execution. Broken notification delivery
must wake/terminate the control path through explicit failure/EOF, not leave
an indefinite wait or add a periodic polling fallback.

## EOF, teardown and failures

EOF between frames in unbound/idle state ends the session normally. EOF in an
incomplete frame is a truncated-frame failure; EOF while active/completing is
transport loss for the active invocation. Half-close is not a graceful-drain
command: EOF starts teardown and no subsequent RESULT delivery is promised.

On EOF, socket failure, fatal protocol error or execution ERROR, close admission
first. If active, request cooperative STOP, await execution and notification
completion, destroy the session on its owner thread, then join that thread and
release proxy/descriptors. Idle teardown also destroys and joins the owner.
Destroy cannot free executable state while execution is active. Session/cache/
arena destruction releases the lease; no wire DESTROY command is defined.

Rust panic, worker invariant failure or generated-code memory fault is a host
failure, never a guest exception or recoverable guest status. The helper must
terminate/wake pending control work, not catch and continue or recreate the
worker and replay. A terminal ERROR is only possible when the host can still
report an ordinary typed failure safely.

The launcher reports startup, handshake, framing, transport and helper-exit
failures. On failure it requests helper termination with SIGTERM, waits up to
500 ms, then sends SIGKILL if needed, and reaps the helper. This external
termination policy handles an unresponsive process; the protocol establishes
no in-process execution deadline and never frees live JIT state to meet one.

## Compatibility and excluded features

Control v1 does not change v0 frames, ABI id, `od_fragment_*`, v0 status/output
semantics, cross-thread sequential handle use or release-on-return guarantees.
New session ABI symbols and their exact ownership/status precedence are a
separate prerequisite; wire field similarity is not permission to retain a
Session in an existing v0 fragment handle.

This profile excludes multiple sessions/codes, code replacement/clear,
overlapping RUN, implicit architectural-state continuation, memory execution,
MMU/SMC/DMA, shared caches, owner-thread migration, reconnect and automatic
retry/replay. Changing peer-visible lifecycle or encoding requires an explicit
protocol-version decision. Neither this design nor its future implementation
alone establishes performance or production VM readiness.
