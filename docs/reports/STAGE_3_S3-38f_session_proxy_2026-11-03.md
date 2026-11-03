# S3-38f: persistent session C ABI proxy

**Status:** bounded code-only implementation slice; exact runtime and harness
gates are added but remain unexecuted.

## Implemented boundary

`crates/od-host/src/session_abi_v1.rs` exports the additive V1 session ABI
declared by `include/open_desktop/session_abi_v1.h`. The opaque handle owns a
dedicated Rust owner thread and synchronized invocation state. `Session` is
constructed, used and dropped inside that thread closure; it is never marked
`Send` or `Sync`. Create validates code admission and first entry RIP before
starting the owner, and reports the existing process MAP_JIT lease conflict as
BUSY. On supported macOS ARM64, capabilities advertise only the S0-09 profile.

The proxy supports one active request. Begin advances the request-ID
high-water mark before state/code/entry/resource checks, copies the request
values, compares code with the immutable session copy, and creates a new
CLOEXEC pipe for each accepted invocation. Its read descriptor transfers to
the caller; the owner exclusively owns the writer. STOP is stored per request
and accepted only while its matching request is still active. The owner stores
the terminal result before writing the completion byte and closing the writer.
Wait blocks on a condition variable until notification closure, writes output
only for success, and consumes a terminal invocation once. Destroy rejects an
unconsumed request and joins the owner after a drain.

The existing `od_fragment_*` implementation and header were not changed. No
dependency was added; the implementation uses Rust standard-library threads,
channels, synchronization and owned file descriptors with the host POSIX
`pipe`/`fcntl` APIs.

## Added lifecycle gates

The macOS ARM64 unit gates are pinned in the Stage3 harness:

- `s3_38f_abi_create_preflight_runs_before_map_jit_lease` checks invalid entry
  rejection before a valid session can acquire the mapping lease.
- `s3_38f_proxy_stop_request_ids_and_destroy_busy_are_exact` checks active
  matching/wrong STOP IDs, wrong wait IDs, output preservation, STOP completion,
  ID reuse rejection and BUSY teardown.
- `s3_38f_proxy_reuses_code_with_fresh_state_and_exclusive_lease` checks the
  process-wide single-session lease and fresh input state across sequential
  same-code invocations.
- `test_s3_38f_session_proxy_gates_are_pinned_once_and_fail_closed` pins exact
  gate identities and commands, source fingerprint inclusion, manifest
  rejection, missing/zero-match evidence rejection and argv rejection.

Independent Sol/high static review passed with no remaining concrete findings.
No Cargo command, Rust test, formatter, Python harness/self-check, build,
layout check, Mini or M1 run was executed for this slice. The gate behavior,
ABI linking/layout on the target toolchain, pipe-failure injection and runtime
race ordering therefore remain unverified. These deferred checks are not
acceptance evidence.

## Scope that remains

This is only the Rust runtime/ABI half of S3-38f. It does not change VMHost
Swift transport, implement protocol V1 event-loop behavior, map socket loss or
worker failure into helper-process termination, or demonstrate packaged
Hardened Runtime behavior. Those are later integration gates; this report
makes no claim that the persistent session is ready for VMHost use or makes a
performance claim.
