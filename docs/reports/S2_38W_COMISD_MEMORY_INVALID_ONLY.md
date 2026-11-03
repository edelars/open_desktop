# S2-38w — COMISD m64 memory invalid-only profile

## Scope and implementation

Added a separate immutable schema-v2 profile,
`s2-long-comisd-memory-rax-invalid-v1`, for exact Long64
`66 0F 2F 00` (`COMISD XMM0,m64 [RAX]`). The fixture requires one executable
code mapping, one read-only eight-byte data mapping at RAX, budget one, stop
RIP at entry+4, and an independent control echo with SSE2, CR0.EM, CR0.TS,
CR4.OSFXSR and CR4.OSXMMEXCPT.

The interpreter reuses the existing binary64 comparison profile with
`quiet_nan_raises_ie=true` and `reject_subnormals=true`. Thus QNaN and SNaN
both raise IE; masked IE commits comparison flags, while unmasked IE commits
only sticky IE and reports `#XM` or arithmetic `#UD` based on OSXMMEXCPT.
After one successful memory read, any subnormal operand causes atomic
Unsupported, including a NaN/subnormal pair. Fetch, exact-form, prerequisite,
AC, canonical-span and read failures precede this post-read refusal. S2-37co's
profile and controls remain unchanged. JIT/native/C ABI/generic/fragment routes
remain closed. ISSUE-033 remains open for auditing older COMISS mixed-event
profiles.

Six literal fixtures cover finite comparison, masked QNaN and SNaN, unmasked
SNaN `#XM`, unmasked QNaN arithmetic `#UD`, and QNaN plus subnormal refusal.
An independent raw-u64 runner oracle covers both NaN classes and operand sides
across IM × OSXMMEXCPT × all 64 prior sticky histories (1,024 cases), while RC
cycles through all four values. It also checks QNaN+SNaN in both orders,
finite/infinity/signed-zero and binary64 boundaries, prior IE alone, memory
read-only validation, result/control tampering and backend denial. Direct ISA
tests cover CPL0–3 and atomic state, gate, combined EM+TS precedence,
form/fetch, successful unaligned access with AC=0, AC=1 refusal, span and
read-error behavior.

## Verification

- `cargo fmt --all -- --check` passed.
- `cargo check --workspace --all-targets --offline` passed.
- `cargo test -p od-isa --offline --quiet`: 1,932 passed.
- `cargo test -p od-testkit --offline --quiet`: 455 passed.
- `cargo test -p od-runner --offline --quiet`: 130 unit tests and 540 fixture
  tests passed.
- `graphify update .`: 12,145 nodes, 26,235 edges, 368 communities.
- `python3 tools/check_layout.py` could not complete in the linked worktree: it
  treats the worktree's `.git` pointer file as an unmapped root file and the
  isolated checkout lacks two ignored Stage 1/2 oracle manifests referenced
  by the pre-existing `STAGE_2_REPORT.md`; no layout files were changed.
- Physical x86 acceptance was not run; this profile is interpreter-only.
- Parent sequential implementation review is clean. An independent Sol/high
  reviewer could not be started because the thread's agent slots were full.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38w--exact-comisd-memory-invalid-only-profile),
[Stage 2 plan](../plans/STAGE_2.md), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38w-comisd-memory-invalid-only-profile).

## Main integration

Integrated on main as `a305d50` plus merge `3ee1999`. The paired S2-31ed/S2-38w
main verification passed: od-isa 1,933, od-testkit 455, od-runner 130 unit +
541 fixture tests, workspace all-target offline check, formatting and layout.
Graphify after integration reports 12,113 nodes, 26,303 edges and 387
communities. The parent sequential code review found no remaining blocking
defects; a separate Sol/high reviewer could not be started because the agent
thread limit was reached. S2-37co and ISSUE-033 remain unchanged/open, and no
physical x86 acceptance is claimed.
