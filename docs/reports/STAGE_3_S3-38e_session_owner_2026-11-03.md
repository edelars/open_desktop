# S3-38e: bounded opt-in S0-09 session owner

**Status:** implemented as a local Rust ownership slice; correctness gates are
added but were not run at the user's request.

## Scope

`od-host::session::Session` owns a copied immutable fragment and its full
fixture decode admission, fixed S0-09 execution-context/dependency identity, a
bounded `MultiEntryBlockCache`, and one `PublishedArenaBackend`. Calls borrow a
fresh `LongModeState`, instruction budget, and STOP atomic. The session stores
no invocation pointers, and cached artifacts remain owned by the existing
compiled-block/cache and arena-handle contracts. Repeated calls over the same
code can reuse the publication.

The session is `!Send`/`!Sync` through its `Rc`-based arena/cache handle and is
intended to be created, run, cleared, and dropped on one thread. `clear` drops
cached handles and the backend, releasing the process MAP_JIT lease; a later
call may reacquire it and reports acquisition errors explicitly. While a
session retains the lease, legacy and other publication attempts keep the
existing `MappingAlreadyLive` behavior. Dispatch errors are returned to the
caller; the session never retries a failed call.

## Verification gates added

The macOS ARM64 local tests in `crates/od-host/src/session.rs` cover:

- repeated same-code execution reuses one publication while fresh CPU state,
  budget, and STOP values are supplied per call;
- `clear` drops the resident entry and releases the lease for another session;
- invalid entry RIP is surfaced as a dispatch error without an automatic retry
  or publication.

All four gates use the existing process-wide `od-host` JIT test lock so their
MAP_JIT lease assertions do not race other host tests.

These tests, formatting, build, layout checks, VMHost IPC, and Mini/M1 checks
were not run for this author slice. The implementation makes no performance
claim.

## Boundaries

This does not route RUN/session lifecycle through VMHost IPC or change V0 ABI,
`FragmentInstance`, per-call publication lifecycle, or the existing sequential
cross-thread V0 guarantees. It does not add `od-machine`, MMU ownership, SMC or
DMA coherence, active-code retirement, or concurrent session execution. It is
not evidence of production VM ownership or multi-process session integration.
