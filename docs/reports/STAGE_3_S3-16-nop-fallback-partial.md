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
