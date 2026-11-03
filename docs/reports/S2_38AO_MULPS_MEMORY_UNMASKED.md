# S2-38ao — MULPS m128 memory unmasked exceptions

Implemented exact Long64 `0F 59 00` `MULPS XMM0,m128 [RAX]` as a bounded
interpreter-only profile with dedicated controls and schema-v2 validation. The
existing masked-memory and register profiles remain separate. The profile
requires SSE bit 25, one read-only 16-byte memory read, and 16-byte alignment
regardless of AC. Complete fetch and exact-form checks precede feature/control
gates; noncanonical fetch raises `#GP(0)` before those gates.

The lane arithmetic uses the independent integer-only raw-u32 oracle against
the actual memory RHS. IE/DE events aggregate before masks; unmasked pre-events
commit the applicable status bits and suppress post-events, XMM0, RIP and
retirement. Post-event OE/UE/PE follow, with unmasked PE preserving status but
suppressing XMM0 and retirement. Fixtures and the bounded profile enforce
`DAZ=FTZ=0`, `OM=UM=1`, and supported MXCSR bits while allowing all RC, IM, DM,
ZM and PM settings and prior sticky histories.

The interpreter ran 76,832 lane-class/mask/delivery cases plus 73,728 rounding,
sticky-history and cross-lane cases (150,560 total oracle comparisons). The
vectors exercise exact and inexact tiny results, UE+PE including a tiny WPR
result rounding to minimum normal, overflow, signed zero, NaN priority and
lane permutations, pre-event suppression across lanes, and masked DE followed
by unmasked PE. A normal p=24 halfway product explicitly checks RN/RU rounding
up and RD/RZ rounding down. Six literal fixtures cover success with prior sticky IE,
unmasked IE/DE/PE, masked UE+PE, and noncanonical-fetch priority.

Focused direct ISA checks cover SSE/EM/OSFXSR and TS ordering, CPL 0–3,
truncated and noncanonical fetch, unsupported forms, malformed MXCSR,
AC-independent alignment, noncanonical data span, exactly one memory read and
state rollback. Testkit schema/result tampering, CLI fixture execution and
interpreter-only backend denial are covered. No physical x86 acceptance is
claimed.

## Validation

Validation passed: `cargo test -p od-isa --offline` (1,969 tests),
`cargo test -p od-testkit --offline` (461 tests), and
`cargo test -p od-runner --offline` (158 unit and 566 fixture tests).
`cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check` passed. Graphify refreshed
the worktree index (12,696 nodes, 29,364 edges, 392 communities). Layout
checking reported only the linked-worktree `.git` entry and two pre-existing
missing manifest links in `STAGE_2_REPORT.md`; no new layout issue was found.
Independent Sol/high review passed. The reviewer rechecked the normal p=24
halfway outputs for all rounding modes, MXCSR and exception metadata tampering,
and the 150,560-case event/boundary coverage. The S2-38ao plan row is complete.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38ao--exact-mulps-memory-profile-with-unmasked-exceptions),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[stage plan](../plans/STAGE_2.md#s2-38ao--exact-mulps-memory-form-with-unmasked-exceptions).
