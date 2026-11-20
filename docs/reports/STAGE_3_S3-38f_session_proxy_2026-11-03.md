# S3-38f: persistent session C ABI proxy

Bounded Rust proxy tests and the signed native C smoke passed. Full S3-38 and
Stage 3 acceptance remain open.

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
All three exact macOS/ARM64 Rust lifecycle gates passed 1/1, and
`test_s3_38f_session_proxy_gates_are_pinned_once_and_fail_closed` exited 0.
These initial results cover the Rust proxy tests and pinned harness identity.
The documented Mini endpoint `senya@192.168.1.14:22` timed out before any remote
command or test ran. The subsequent native C checks are recorded below.

## Scope that remains

This is only the Rust runtime/ABI half of S3-38f. It does not change VMHost
Swift transport, implement protocol V1 event-loop behavior, map socket loss or
worker failure into helper-process termination, or demonstrate packaged
Hardened Runtime behavior. Those are later integration gates; this report
makes no claim that the persistent session is ready for VMHost use or makes a
performance claim.

## Native C client and a lost notification reader, 2026-11-19

The same C11 client compiled against the public V1 header and linked to both
ARM64 `od-host` static libraries with minimum macOS 14.0. Compile-time checks
cover the public struct sizes, alignments and field offsets. Both client
executables passed strict signature verification with Hardened Runtime and
the project’s `allow-jit` entitlement.

On local macOS 26.7.1 (25G241), Xcode 27.1 and Rust 1.99.0, both builds
delivered one completion byte followed by EOF and returned valid CPU state.
The next invocation was held active with a same-code guest loop. The fixture
then intentionally violated the read-descriptor lifetime to inject reader loss
before sending explicit STOP. With default, unblocked `SIGPIPE`, the client
built from `ef44df6` was terminated by signal 13. The fixed build exited 0.

The macOS pipe writer uses Darwin `F_SETNOSIGPIPE` so a lost reader reaches
the existing EPIPE handling. The test verifies `INVALID_STATE`, unchanged
output, failed admission, successful destroy and a fresh session after the
mapping lease is released. The client confirms that the process-wide
`SIGPIPE` disposition is still the default. The caller must keep the read end
open until wait returns in normal use. [ISSUE-524](../KNOWN_ISSUES.md#issue-524)

### Reproduce the native smoke

Run from the repository root on macOS ARM64 with a prepared offline Cargo
cache. The build uses the same smoke entitlements as VMHost.

```sh
export MACOSX_DEPLOYMENT_TARGET=14.0
target_dir=$(mktemp -d)
cargo build --locked --offline --release -p od-host --lib \
  --target aarch64-apple-darwin --target-dir "$target_dir"
xcrun clang -target arm64-apple-macos14.0 -std=c11 -Wall -Wextra -Werror \
  -I include tests/system/session_abi_v1_smoke.c \
  "$target_dir/aarch64-apple-darwin/release/libod_host.a" \
  -ldl -lpthread -lm -framework Security -framework SystemConfiguration \
  -framework CoreFoundation -framework CoreServices -framework Foundation \
  -o "$target_dir/session-abi-v1-smoke"
codesign --force --sign - --options runtime \
  --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements \
  "$target_dir/session-abi-v1-smoke"
codesign --verify --strict "$target_dir/session-abi-v1-smoke"
"$target_dir/session-abi-v1-smoke"
```

### Pipe creation under descriptor exhaustion

The signed C client also exercises pipe creation with one available file
descriptor. It temporarily reduces its own `RLIMIT_NOFILE`, fills the
descriptor table with owned descriptors and confirms that raw `pipe()`
returns `EMFILE`. The public begin returns `ALLOCATION_FAILED` without
writing the token. STOP and wait report `NO_ACTIVE_RUN`, output remains
intact, and the remaining descriptor slot can be allocated again.

After the client restores its limit, the same request ID is rejected.
A greater ID completes with normal notification and state. Both the
`ef44df6` and fixed signed libraries passed this part on local macOS
26.7.1. The fixture executes it before the lost-reader test.

A separate signed probe showed that `F_SETFD` and `F_SETNOSIGPIPE` on an
existing pipe still succeed with a full descriptor table. Failure injection
for those setup calls remains open.

### Confirmation on M1 Pro

Apple M1 Pro, macOS 14.6.1 (23G93), Xcode 15.4 and Rust 1.99.0 ran the
same four signed native client cases. The baseline linked the `ef44df6`
library. The fixed variant used the same source export with the session
notification change from `69e8b57`. Both C fixture versions verified
normal notification before injecting reader loss. The baseline exited by
SIGPIPE and the fixed variant exited 0 with either fixture. With the fixture
from `3179068`, both builds also passed the earlier `EMFILE` recovery
checks. The archive includes both hosts’ receipts, source and executable
hashes, and the three local Rust lifecycle results.

Archive SHA-256 `a7e142844bf84c23b6aebdf03215d3e5392f2e066120fefe2de688931b3a9b53`.
[Native session ABI evidence](data/stage3/s3-41-m1pro-de1f446-20261119/native-session-abi.json.gz)

### Concurrent wait on one active RUN

The signed C fixture starts two pthreads on the same live request. The guest
loops until the main thread sends STOP. A condition variable lets the main
thread observe the first waiter return before STOP. That waiter returns
`BUSY` with its result buffer intact. Another `begin` also returns `BUSY`
with its token buffer intact and consumes the supplied request ID.

After STOP, the other waiter receives the sole result with the expected
guest state. The client reads one completion byte and EOF, and a repeat wait
returns `NO_ACTIVE_RUN`. It checks rejection of the consumed request ID and
acceptance of a greater ID. Both waiters are joined before destroy.

On local arm64 macOS 26.7.1 with Xcode 27.1 and Rust 1.99.0, the extended
fixture linked to the exact `963a0f6` `od-host` release library. The executable
verified with Hardened Runtime and `allow-jit` at deployment target 14.0.
Ten consecutive runs passed, including the existing `EMFILE` and lost-reader
cases. The native build and execution commands above reproduce this fixture.
[Local receipt and source hashes](data/stage3/s3-38f-native-waiters-20261119/local.json)

A controlled build with the existing-waiter `BUSY` branch removed reached
the parent's five-second watchdog. The same fixture passed against the
pinned library.
[Controlled regression receipt](data/stage3/s3-38f-native-waiters-20261119/negative.json)
