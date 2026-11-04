# ADR-0004: Bounded checked-memory cache handle and lifetime

- **Date:** 2026-11-01.
- **Status:** accepted for the bounded S3-18e/f fixtures.
- **Related documents:** [JIT contract](../specs/JIT_CONTRACT.md#s3-18e-capacity-one-checked-memory-cache-fixture-partial), [Stage 3 plan](../plans/STAGE_3.md), [S3-18e report](../reports/STAGE_3_S3-18e_2026-11-01.md), [S3-18f report](../reports/STAGE_3_S3-18f_2026-11-03.md).

## Question

Which exact frozen checked-memory profiles may retain a published artifact for
same-key reuse without changing the existing uncached executor's
release-on-return contract or implying production memory ownership?

## Context

`MemoryBlockExecutor` publishes, executes, validates and releases one mapping
before returning. A cache that keeps a checked-memory mapping resident cannot
implement that contract without changing its semantics. Existing cache keys
and dispatch admission are scoped to other fixture profiles; identifying a
memory pair as S0_09 would permit accidental cross-profile reuse. The original
S3-34a scope is exactly `StoreThenLoad` and `LoadThenStore` under immutable
fixture source, dependency and execution-context snapshots. S3-26e separately
defines one exact `LoadThenLoad` sequence, `48 8B 03 48 8B 0B`, whose two
same-address reads must remain independent. S3-18f evaluates only whether that
exact profile can use the same retained-handle boundary. Production MMU
ownership, live mutation, asynchronous compilation and active-code retirement
are not available boundaries for this slice.

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
policy and a memory-specific typed key/profile. The key distinguishes exactly
`StoreThenLoad`, `LoadThenStore`, and `LoadThenLoad` and includes the complete
immutable source view and entry RIP, validated executable dependency
identities/generations and permissions, and a nonzero caller-owned
execution-context generation/profile. The `LoadThenLoad` admission is limited
to the exact S3-26e bytes and typed sequence; `StoreThenStore` and all other
profiles remain rejected by the retained path.
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
  behavior for the three explicitly admitted profiles while existing uncached
  dispatch remains available as the correctness reference.
- Source, dependency or context generation changes cannot reuse the old entry.
  The fixture caller is responsible for keeping each snapshot immutable during
  a synchronous operation and advancing generations without wrap.
- Tests must compare full CPU state, RAM, callback trace, retirement, budget and
  exit behavior against uncached dispatch, including control handoffs and a
  committed-write continuation with no replay. For `LoadThenLoad`, callbacks
  remain two distinct reads even at the same address; a second-site fault or
  backend error preserves the first load at RIP `A+3` and cannot replay it.
  Entry at internal RIP `A+3` remains rejected for the retained pair, while
  existing guard priority is unchanged. Runtime CPU/RAM/helper/STOP/event/
  budget/stop-RIP bindings are supplied afresh on each hit.
- This does not implement an MMU/owner, mutation notification protocol, SMC/DMA
  coherence, asynchronous compilation, active-code retirement, multi-entry
  eviction policy, or production routing. It yields no performance or default
  policy conclusion; benchmarks and Mini checks remain deferred.

## Revisit when

Revisit this boundary before adding production owner/MMU integration,
asynchronous compilation, any further memory profile beyond these three exact
sequences, or more than one resident entry. S3-18f authorizes only its exact
`LoadThenLoad` profile; future forms require new analysis rather than
generalizing this fixture contract by default.
