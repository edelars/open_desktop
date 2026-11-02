# S2-30df — large-gap `FADD ST(0), ST(i)` precision

The Long64 interpreter now admits bare `D8 C0..C7` sums with normalized
Normal-to-Normal exponent gaps through the maximum gap of 32765. The forward
form explicitly opts into S2-29af's existing large-gap precision helper.
Reverse `DC` and pop `DE` FADD keep their gap≤64 preflight; the arithmetic
helper and FSUB/FSUBR paths are unchanged.

All twelve all-exception-masked PC24/53/64 FCWs remain supported. Operand and
result admission remains canonical nonzero Normal with Valid tags. PE stays
sticky, C1 comes from the helper's pre-carry magnitude increment diagnostic,
and successful execution preserves TOP/tags/source and unrelated state while
setting FIP and advancing RIP by two. Overflow or any other unsupported
result refuses atomically. Register execution performs no data access.

Coverage adds literal cases at gaps 65, 66, 127, 128 and 32765, both operand
magnitude orders and signs, every FCW, every TOP and source index, and full
state comparison. Existing gap≤64 rounding tests remain. Additional cases
cover D8 alias doubling, directed negative rounding, p53 midpoint neighbors,
overflow refusal, and atomic DC/DE gap-65 rejection. The schema-v2 runner has
a literal gap-65 fixture with an actual `2^-65` RHS; it checks full x87 state
and JIT denial. No fixture schema change was needed.

Validation in the author worktree:

- Focused `cargo test -p od-isa fadd --offline`: 26 tests passed.
- `cargo test -p od-isa -p od-testkit -p od-runner --offline`: od-isa 1,862,
  od-testkit 455, runner unit 120, runner fixture 507 passed.
- The new fixture test passed and confirms JIT compilation remains denied.
- `cargo fmt --all -- --check`, workspace all-target offline check and diff
  check passed. `graphify update .` refreshed the graph (11,618 nodes, 25,020
  edges, 370 communities).
- `python3 tools/check_layout.py` reported the isolated worktree's `.git` marker
  as unmapped and two pre-existing broken links from the shared Stage 2 report
  to `work/` manifests absent from this worktree. The implementation adds no
  paths outside existing directories; main-tree layout validation remains for
  integration.
- Independent Sol/high review: pending. No physical x86/JIT execution claim.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cp-bounded-fadd-st0-sti-d8-c0c7),
[Stage 2 plan](../plans/STAGE_2.md#s2-30df-large-gap-fadd-st0-sti-admission),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), the existing
[S2-30cp report](S2_30CP_X87_FADD_PRECISION.md), and [ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
