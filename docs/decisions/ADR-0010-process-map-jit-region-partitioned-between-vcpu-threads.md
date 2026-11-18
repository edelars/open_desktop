# ADR-0010: Process MAP_JIT region partitioned between vCPU threads

- **Date:** 2026-11-18.
- **Status:** proposed; accepted with the S6-42 gate review. Records the S6-40 contract decision (accepted 2026-11-18) and what S6-42 implemented; written by the S6-42 executor, to be confirmed or replaced by the coordinator.
- **Related documents:** [JIT contract, S6-40 section](../specs/JIT_CONTRACT.md#s6-40-кэш-кода-при-нескольких-vcpu-контракт) (rules A1 to A7, M1 to M6, S1 to S6), [JIT contract, S6-42 section](../specs/JIT_CONTRACT.md#s6-42-межпроцессорная-инвалидация-и-безопасное-освобождение-блоков), [ADR-0004](ADR-0004-capacity-one-memory-cache.md), [ADR-0005](ADR-0005-persistent-same-code-session.md), [ISSUE-038](../KNOWN_ISSUES.md), [Stage 6 plan](../plans/STAGE_6.md).

## Question

How do several vCPU threads of one process publish, execute and free JIT blocks when the process may own only one `MAP_JIT` region and the block handles are `Rc`-based, hence `!Send`?

## Context

Apple allows a process with Hardened Runtime and the allow-jit entitlement one `MAP_JIT` region (ISSUE-038). `od-host` rejects a second region as `MappingAlreadyLive`; before S6-42 every admission of `PagedFetchedLoadSession` mapped a region of its own and held the process lease until the block was dropped, so two vCPU threads could not both run native code. ADR-0004 (capacity-one fixture cache) and ADR-0005 (persistent session on one owner thread, one lease per process) describe one executable-memory owner and one thread; neither describes several threads. The S6-40 contract chose a private code cache per vCPU (option (a)) over a shared cache with deferred reclamation (option (b)), because (a) keeps the S3-25c life cycle and the `!Send` rule and needs no cross-thread release.

## Options

1. One cache shared by all vCPU threads, handles `Send + Sync`, release after a quiescent state of every vCPU. Rejected until a measurement shows the need (S6-37, S6-38): it needs epoch accounting, cross-thread code synchronisation and handles without `Rc`.
2. One region per vCPU. Rejected: the platform allows one region per process.
3. One region per process, owned by one `Sync` object of `od-host`, cut into disjoint parts, one part per vCPU thread; the part becomes the `!Send` arena of that thread. Selected.

## Decision

- **Owner (M1).** One object, `od_host::jit_region::SharedJitRegion`, maps the region and holds the process lease. It is `Sync`; threads share a reference inside `std::thread::scope` (or hold parts). The mapping is released only after the owner and every part are dropped. While it lives, any other owner (`PublishedCode::publish`, `PublishedArenaBackend`, a session without a part, the v0 fragment ABI) gets `MappingAlreadyLive` (M6). Refinement of M1: the mapping and the lease live while the owner or any part lives (`Arc<RegionShared>`), not "the lease ends after `join`"; this is safer (use-after-unmap is excluded). Refinement of ADR-0005: its "one executable-memory owner" means one lease holder (`SharedJitRegion`); the parts are arenas inside it, bound to threads.
- **Parts (M2).** The region is cut into `parts` disjoint parts of `slots_per_part` slots of `ARENA_SLOT_BYTES`; each part starts on a system page. A part is handed out once (`take_part`) as a `Send` value without executable handles. Parts are never rebalanced and no thread takes a slot of another part while threads run. A part that is full refuses the publication (the session falls back to the interpreter). **Deviation from M2, not implemented:** the contract said "if a part has no room, its owner evicts its own entries (FIFO)"; there is no eviction (`jit_region.rs`, `allocate_and_write`). Unreachable for a session with one block; open for several blocks per part.
- **Arena of the thread (A1).** `JitRegionPart::activate` makes the part an `ActiveJitPart` (`Rc`, `!Send`) in the thread that will own it. The slot table, the free list and the slot generations live there. Blocks (`PublishedCode`), handles, PC-maps and entry pointers do not cross threads; no `unsafe impl Send/Sync` is added for them. The only `unsafe impl Send/Sync` is on the internal region value (a raw pointer used to compute addresses of disjoint slots).
- **Write and release (M3, A4).** Only the owner thread writes a slot (its own write window of `pthread_jit_write_protect_np`, which acts on the calling thread) and only the owner releases it, when the `PublishedCode` that owns the slot is dropped (types forbid another thread). There is no grace period: no other thread can enter a block of this part. A release deferred because the slot table was borrowed is applied at the next safe point of the session: this is an implemented mechanism, but its path (`try_borrow_mut` failing) is defensive, unreachable in the current code and not covered by tests; the `pending_release == 0` checks are trivial. A reused slot gets a larger generation. In test builds only (`cfg(test)`) a released slot is filled with `BRK`, and only the first 64 of 4096 bytes: a limit on the evidential strength.
- **Visibility of new code (M4, M5).** After the write: `sys_icache_invalidate` of the written range, then an explicit `ISB` on the writing and executing thread. M4 applies to slots: the explicit `ISB` exists only on the slot path (`publish_*` with a part); the `Owned` branch of `publish_with_metadata_in` has no `ISB`, as before. Writer and executor are the same thread, so cross-thread publication (M5) is not used. Whether `sys_icache_invalidate` already ends with `ISB` is not verified; the explicit barrier makes it irrelevant.
- **Invalidation between vCPUs (A3, D1).** Only through the shared page generations of `commit_write`: every entry (every step of the one-instruction production profile) rereads the instruction bytes and the generation and compares them with the admission key of the block. Delay: the first entry after a write sees the new generation when synchronised through a lock (the bus is behind a `Mutex`) or a channel; without the lock it is not verified (S6-42b). **A7 is not met (bus behind a `Mutex`): the JIT step runs under `Mutex<MachineBus>`, the detection delay without a lock was not tested — S6-42b.** So the A/B scenario, the free run and `stop_all` check the interleaved mode at lock granularity; D1 and G-XMC in parallel mode are not proven. The check "block bytes and generation equal the page" is nearly tautological (the fetched code and generation come from a fresh snapshot under the same lock, not from the key of the executed block); the evidence is the effects (RAX, data qword for load and store on the JIT-only path).

## Consequences

- The same code is compiled once per vCPU that runs it, and the region is N times bigger; the cost is measured in S6-37 and S6-38, and option 1 is reconsidered only on those numbers, with a new ADR.
- Fixture backends (`PublishedArenaBackend`, `MultiEntryBlockCache`) are not moved to slots; they stay single-thread and keep their own region (A5).
- Multi-instruction blocks with stores in parallel mode need S3-24; links between blocks need D1 to D3 of the contract (S3-31).
- The shared interpreter and the paged JIT path still take `&mut MachineBus`, so the slice runs vCPU threads over one bus behind a lock; a shared-reference variant is open (S6-42 report, limits).
- Sources marked "from memory" in the S6-40 contract (Intel SDM 9.1.3, Arm ARM cache maintenance, Apple rule on `pthread_jit_write_protect_np`) are still to be verified before the acceptance of S6-42.

## Revisit when

- A measurement (S6-37, S6-38) shows that the per-vCPU recompilation or the N-times larger region costs more than a shared cache (option 1).
- A part runs out of slots in practice, and the FIFO eviction of M2 has to be implemented.
- S6-42b removes the bus `Mutex` around the JIT step (A7) and the detection delay without a lock can be measured.
- The platform allows more than one `MAP_JIT` region per process.
