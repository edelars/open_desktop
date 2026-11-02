# S2-37dh MINSS m32 memory implementation report

Date: 2026-11-01

## Implemented scope

The interpreter and schema-v2 runner admit only the exact Long64 encoding
`F3 0F 5D 00` for `MINSS XMM0,m32 [RAX]`, through the dedicated profile
`s2-long-minss-memory-rax-v1`. Execution fetches the four-byte instruction,
checks SSE and control gates in the established order, validates the complete
canonical48 span `[RAX,RAX+3]`, and performs exactly one four-byte memory read.
With RFLAGS.AC set, this bounded profile refuses locally before address
validation or data access; with AC clear, it admits an unaligned m32 read.
Read failures leave CPU state, MXCSR, RIP and retirement unchanged.

The local MXCSR contract requires supported mask `0xffbf`, all six exception
masks set, DAZ and FTZ clear, and reserved bits 16–31 clear. All four RC values
and all previous sticky histories are accepted. The implementation reuses
`Binary32::min_sse_raw_result` and changes only XMM0.low32 plus generated IE/DE
sticky bits. Numeric ties, both signed-zero orders, and either QNaN or SNaN
select the raw m32 operand; SNaN payload bits are not quieted. NaN sets IE and
suppresses DE for this scalar operation. Otherwise a subnormal operand sets
DE. XMM0.high96 and all unrelated state remain unchanged.

The testkit oracle derives binary32 class and ordering from raw integer bits,
independently of production helpers and host floating point. Direct ISA checks
exercise 10 literal operand pairs over every RC and all 64 previous sticky
histories (2,560 comparisons), including signed-zero and equal-value RHS
selection, both NaN source positions, SNaN payload preservation, denormal
status, and IE-over-DE behavior. Schema and runner checks cover exact-code and
control/result tampering, poisoned XMM0, backend denial, fault precedence,
canonical span and endpoint overflow, AC policy, one-read/no-write behavior,
and failed-read atomicity. Generic, fragment, JIT, native, and C ABI execution
remain closed for this profile.

## Verification

- `cargo test -p od-isa minss_memory_rax --offline`: 2 passed.
- `cargo test -p od-isa --offline`: 1,780 passed.
- `cargo test -p od-testkit --offline`: 454 passed.
- `cargo test -p od-runner --offline`: 112 unit and 489 fixture tests passed.
- `cargo test -p od-x86-runner --offline`: 8 library and 4 binary tests passed.
- `cargo test -p od-runner --test fixture_v2 minss_m32_memory_literal --offline`: 1 passed.
- `cargo fmt --all -- --check` and `cargo check --workspace --all-targets --offline` passed.
- `git diff --check` passed. `python3 tools/check_layout.py` reported only the
  existing `.git` worktree marker and two unavailable historical oracle manifests
  tracked by ISSUE-023.
- `graphify update .` passed: 11,136 nodes, 24,046 edges, 337 communities.

No physical x86 acceptance is claimed. The separate Linux native backend
target is not evidence of this local interpreter profile.

Sources: Intel SDM Rev. 093 Vol. 2B `MINSS` pp. 4-24–4-25; Vol. 1 §4.9.2,
§§11.5.2.1–.2 and Appendix D; Vol. 2A Type 2/Table 2-19; Vol. 3A §16.1.3 and
Table 16-1.

## Review

Independent Sol/high review is clean. The review found that the first integration
had replaced the existing MAXSS oracle hook, omitted MINSS non-interpreter
snapshot provenance denial, and retained a copied MAXSS encoding diagnostic.
The MAXSS control echo and independent result validator are restored as a
separate profile branch; MINSS now permits non-interpreter results only as
Unsupported with an unavailable snapshot; the diagnostic names `F3 0F 5D 00`.
Regressions prove that forged JIT/native results with available success,
exception, or AC-refusal snapshots fail validation, while the expected native
Unsupported/unavailable result passes. After these changes, the focused MINSS
and MAXSS fixture tests, full od-testkit and od-runner suites, workspace check,
format, and Graphify update passed.
