# ADR-0004: Bounded checked-memory cache handle and lifetime

- **Date:** 2026-11-01.
- **Status:** accepted for the bounded S3-18e fixture.
- **Related documents:** [JIT contract](../specs/JIT_CONTRACT.md#s3-18e-capacity-one-checked-memory-cache-fixture-partial), [Stage 3 plan](../plans/STAGE_3.md), [implementation report](../reports/STAGE_3_S3-18e_2026-11-01.md).

## Question

How can the two frozen S3-34a checked-memory profiles retain a published
artifact for a same-key reuse without changing the existing uncached executor's
release-on-return contract or implying production memory ownership?

## Context

`MemoryBlockExecutor` publishes, executes, validates and releases one mapping
before returning. A cache that keeps a checked-memory mapping resident cannot
implement that contract without changing its semantics. Existing cache keys
and dispatch admission are scoped to other fixture profiles; identifying a
memory pair as S0_09 would permit accidental cross-profile reuse. The S3-34a
scope is exactly `StoreThenLoad` and `LoadThenStore` under immutable fixture
source, dependency and execution-context snapshots. Production MMU ownership,
live mutation, asynchronous compilation and active-code retirement are not
available boundaries for this slice.

## Options

1. Change `MemoryBlockExecutor` so its published mapping can escape the call.
   This would alter all existing one-shot callers and conceal whether a handle
   is resident or released.
2. Add a separate typed opt-in cache/backend contract with an owned opaque
   published handle, leaving the one-shot executor unchanged.
3. Add a generalized cache shared across memory and register-only profiles.
   This adds profile and lifetime machinery beyond the two admitted forms.

## Decision

Use option 2 for the bounded fixture only. `od-jit` owns a capacity-one cache
policy and a memory-specific typed key/profile. The key distinguishes
`StoreThenLoad` from `LoadThenStore` and includes the complete immutable source
view and entry RIP, validated executable dependency identities/generations and
permissions, and a nonzero caller-owned execution-context generation/profile.
The caller keeps that snapshot frozen through synchronous lookup, compile,
publication, execution and validation. On a miss or clear, the cache drops the
previous inactive published handle before publishing a replacement. A hit
revalidates the full key. The handle remains owned until execution result
validation completes.

`od-host` owns the concrete retained published handle and invokes the existing
typed checked-memory publication and execution APIs. This path is separate
from `MemoryBlockExecutor` and does not weaken its release-before-return
contract. This is a narrow exception to the ordinary `od-host` mapping lifetime
rule: only this opt-in fixture handle may retain an executable mapping between
calls, and it owns code plus immutable artifact metadata only. It never retains
invocation-local CPU, RAM, helper, STOP or event pointers; those values are
borrowed afresh for one synchronous execution and result validation. The unsafe
private synchronous ordinary-RAM preconditions apply on every execution: code
bytes cannot alias mutable data RAM; MMIO, DMA, concurrent observers, re-entry
and unwind through the helper are excluded; writes are all-or-nothing. The key
and API are not `S0_09`, do not enter the production machine, and are not a
generalized cache interface. Failed publication/execution is terminal;
committed guest memory effects are never replayed.

## Consequences and boundaries

- The one-entry cache can establish same-key reuse and bounded replacement/drop
  behavior for the two S3-34a profiles while existing uncached dispatch remains
  available as the correctness reference.
- Source, dependency or context generation changes cannot reuse the old entry.
  The fixture caller is responsible for keeping each snapshot immutable during
  a synchronous operation and advancing generations without wrap.
- Tests must compare full CPU state, RAM, callback trace, retirement, budget and
  exit behavior against uncached dispatch, including control handoffs and a
  committed-write continuation with no replay.
- This does not implement an MMU/owner, mutation notification protocol, SMC/DMA
  coherence, asynchronous compilation, active-code retirement, multi-entry
  eviction policy, or production routing. It yields no performance or default
  policy conclusion; benchmarks and Mini checks remain deferred.

## Revisit when

Revisit this boundary before adding production owner/MMU integration,
asynchronous compilation, other memory profiles, or more than one resident
entry. Those changes require new ownership and retirement analysis rather than
generalizing this fixture contract by default.
