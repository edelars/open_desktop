# S3-16: exact NOP fallback partial

Date: 2026-10-26

## Implemented slice

This local partial adds an opt-in Rust API for a hybrid block/interpreter run
that admits the existing S0-09 forms and exact bare `0x90` only. Admission is
created from a full long-mode decode over an immutable `CodeView`; it owns the
validated bytes and sequential instruction boundaries, and checks branch
targets against those boundaries. It does not alter fixture-v1 admission or
the existing `jit-dispatch` CLI.

When a native block reaches an admitted NOP, the native executor returns and
releases its published mapping before the dispatcher invokes the existing
`od_isa::execute::step`. The instruction fetch view exposes only that NOP's
single byte. Native and interpreter retirements are reported separately and
must equal the initial budget minus the remaining budget. Stop address, shared
STOP flag, code end, and budget are sampled in that order at instruction
boundaries. Errors terminate the call; no retry or success-shaped fallback is
performed.

The API intentionally does not support general Stage 2 interpreter fallback,
memory operations, guest page faults, fixture/CLI expansion, or any instruction
outside S0-09 plus exact bare NOP. S3-16 remains open pending broader planned
coverage and review.

## Verification

- `cargo fmt --all -- --check` — passed.
- `cargo test -p od-jit --offline` — 70 passed.
- `cargo test -p od-host --offline` — 27 passed. On this Apple Silicon host,
  native tests covered MOV/ADD → NOP → CMP/Jcc continuation, flag preservation,
  and STOP observed after a native prefix.
- `cargo check --workspace --all-targets --offline` — passed.
- `git diff --check` — passed.
- `graphify update .` — passed; refreshed the workspace code graph.
- Independent review — clean; reviewer found no concrete defects in admission,
  boundary checks, STOP ordering, terminal-error behavior, or mapping lifetime.

These checks establish the local supported path only. They do not establish
general fallback correctness, physical x86 oracle agreement, or completion of
S3-16.

## S3-16m: bounded-memory dispatcher NOP continuation (2026-10-27)

Added a separate opt-in `od_jit::memory_dispatch::run_with_nop_fallback`.
The existing strict `run` is unchanged and stops at bare `0x90` with
`Unsupported`. The new entry reuses strict dispatch with the current remaining
budget, and steps only decoder-confirmed exact bare `90` once through the
existing interpreter and a one-byte immutable code view. It reports native and
interpreter retirements separately and checks their exact sum against consumed
budget after each stage. Prefix NOP, `0F 1F`, PAUSE and other unsupported forms
are not admitted.

Native/mapping release occurs before the interpreter step: the host
`PublishedMemoryBlockExecutor` decodes the result, explicitly drops the
published block and returns; only then does the dispatcher call interpreter
step. The acceptance sequence pair → NOP → pair proves both native pair
artifacts can publish sequentially under the one-mapping host constraint.
Budgets 0..=5 compare full CPU state, RAM and data callback counts with an
independent full-interpreter run. Additional cases cover STOP and stop-RIP
before/after NOP, continuation from actual RIP without repeating the first
pair, unsupported tail preserving prefix effects, and strict-run rejection.

This remains a bounded unsafe borrowed-memory fixture API. It does not add
production `od-machine` ownership, MMU fetch, or general Stage 2 fallback and
does not complete S3-16.

### S3-16m verification

- Focused `cargo test -p od-host --offline bounded_memory_dispatch -- --nocapture` — 4 passed.
- `cargo test -p od-host --offline --quiet` — 63 unit passed; 12 integration passed; 2 ignored.
- `cargo test -p od-jit --offline --quiet` — 131 passed.
- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets --offline` — passed.
- `cargo check -p od-host --target x86_64-unknown-linux-gnu --all-targets --offline` — passed with existing platform-gated dead-code warnings.
- `git diff --check` and `graphify update .` — passed.

These results establish only the opt-in bounded bare-NOP path and its local
native/interpreter comparisons. No Stage 2 suite, physical Mac mini oracle, or
long benchmark was run; none was needed for this scoped slice.

### Independent review

Review signed off with no open findings. The reviewer independently passed
the focused dispatcher tests, `od-host` and `od-jit` suites, formatting,
workspace/Linux checks, and diff check.

## S3-16n: bounded-memory exact NOT RCX fallback (2026-11-03)

The opt-in `run_with_nop_fallback{,_controlled}` now also admits exact bare
`48 F7 D1` (`NOT RCX`) after strict dispatch reports `Unsupported`. The exact
decoder matcher requires W64 NOT, RCX destination, three-byte length, no other
prefix, and matching source bytes. The dispatcher gives the existing
`od_isa::execute::step` an immutable three-byte fetch view, then resumes with
the actual RIP and remaining budget. Strict `run`, native admission, and all
other unsupported forms remain unchanged. Existing STOP, event, boundary,
budget, retirement, and native mapping release ordering are retained. The host
fixture composes native pair → NOT RCX interpreter step → native pair and
compares complete CPU/RAM state and accounting with the interpreter for budgets
0..=5, stop-RIP before and after fallback, atomic STOP after the native prefix,
and a strict/opt-in NOT RAX neighbor. The od-jit regression also preserves
bare-NOP fallback when code ends at `u64::MAX`.

Rust identities:
`memory_dispatch::tests::not_rcx_fallback_is_exact_and_strict_run_stays_strict`,
`memory_dispatch::tests::nop_fallback_handles_code_end_at_u64_max`, and
`tests::bounded_memory_dispatch_not_rcx_fallback_composes_native_prefix_and_suffix`.
Harness gates: `s3_16_not_rcx_fallback`, `s3_16_nop_u64_max_regression`, and
`s3_16_not_rcx_host_continuation`; fail-closed self-test:
`test_s3_16_not_rcx_fallback_gate_is_pinned_and_fail_closed`. The manifest is
pinned at 308 Rust gates (298 acceptance + 10 lifecycle), 326 local steps, and
139 self-test names. Gates and self-check are encoded and statically reviewed
but unexecuted. No behavior test, build, formatting, Cargo, Python harness,
Mini, or M1 check was run for this addition. Production owner/MMU integration,
broader fallback coverage, and S3-16 completion remain open.
