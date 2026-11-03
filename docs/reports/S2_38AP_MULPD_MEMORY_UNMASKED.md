# S2-38ap — MULPD memory unmasked profile

Implemented the exact Long64 legacy `66 0F 59 00` form
`MULPD XMM0,m128 [RAX]` as schema-v2 profile
`s2-long-mulpd-memory-unmasked-rax-v1`. This slice is local and
interpreter-only. It preserves the existing masked memory and register
profiles.

The runner requires SSE2 CPUID bit 26, CR0.EM=0, CR4.OSFXSR=1 and CR0.TS=0.
It fetches the complete four-byte instruction before gate delivery, checks the
exact form, and faults `#GP(0)` for an incomplete noncanonical fetch before
feature gates. MXCSR is restricted to supported mask `0xffbf`, reserved bits
clear, DAZ=FTZ=0 and OM=UM=1; rounding mode, IE/DE/ZE/PE masks and prior sticky
histories remain configurable. Data access requires 16-byte alignment
independent of AC, a checked canonical48 `[RAX,RAX+15]` span and one atomic
16-byte read.

The two binary64 lanes use the old XMM0 value and the actual memory RHS. The
independent `independent_mulpd` oracle performs integer-only raw-u64 arithmetic.
The implementation aggregates lane events, gives IE/DE pre-events priority
over post-events, preserves applicable status on `#XM`, and suppresses XMM/RIP
commit when an unmasked exception is delivered. Coverage includes exact and
inexact tiny cases, UE+PE, rounding to minimum normal, half-minsub, overflow,
an inexact normal exact-halfway product and products immediately above and
below it, NaN source priority and lane permutations, signed zero, cross-lane IE/DE
suppression of OE/UE/PE, and masked DE followed by unmasked PE.

## Verification

- Direct ISA regression: `cargo test -p od-isa mulpd_memory_rax_unmasked
  --offline -- --nocapture` — 2 passed. Includes OM=0 and UM=0 unsupported
  admission checks, feature/fetch/CPL/alignment/read rollback and exception
  commit/suppression behavior.
- Literal fixture schema and tamper regression:
  `cargo test -p od-runner --test fixture_v2
  mulpd_m128_unmasked_literal_fixtures_validate_state_schema_and_backend_denial
  --offline -- --nocapture` — passed. Six checked-in fixtures exercise success,
  IE, DE, PE, UE+PE with minimum-normal rounding, and noncanonical fetch
  precedence. It also checks schema requirements, state/result tampering and
  JIT/native backend denial.
- Event product: 7×7 event classes × 16 mask combinations × 2 delivery modes =
  1,568 interpreter cases passed.
- Full history/boundary product: 24 vectors plus lane permutations × 4 RC × 64
  sticky histories × 16 mask combinations × 2 delivery modes = 393,216 cases
  passed. For `(0x3ff0000000000001 × 0x3ff8000000000000)`, explicit RC checks
  confirm RN/RU yield `0x3ff8000000000002`, while RD/RZ yield
  `0x3ff8000000000001` (PE set).
- Full package rerun passed: od-isa 1,970; od-runner binary 154;
  `fixture_v2` 566; od-testkit 461. An earlier concurrent full-suite run had
  one CMPSs fixture assertion failure; the exact test passed in isolation and
  the complete rerun passed all 566 fixture tests.
- `cargo check --workspace --all-targets --offline`,
  `cargo fmt --all -- --check` and `git diff --check` passed. `graphify update .`
  completed with no topology changes (12,700 nodes, 29,369 edges, 394
  communities).
- `python3 tools/check_layout.py` reports the isolated worktree `.git` marker
  and two broken historical oracle manifest links in
  `docs/reports/STAGE_2_REPORT.md`; no new layout issue was introduced.
- Independent Sol/high review completed cleanly. The reviewer independently
  checked the CLI normal-tie encodings and PE behavior across all RC values,
  fixture/report counts, profile contracts, and the affected diff. No confirmed
  findings remain.

The profile does not enable native/JIT/C ABI/generic/fragment execution and
does not claim physical x86 acceptance.
