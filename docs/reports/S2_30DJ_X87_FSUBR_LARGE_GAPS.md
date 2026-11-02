# S2-30dj — large-gap `FSUBR ST(0), ST(i)` precision

The Long64 interpreter now admits normalized Normal-to-Normal gaps through
32765 for exact bare `D8 E8..EF` FSUBR. It validates both original operands,
flips the sign bit only in a local copy of ST(0), and adds that value to ST(i)
with the existing S2-29af direct-addition precision helper. This computes
`ST(i)-ST(0)` directly at the requested FCW precision and rounding mode. It
does not negate a rounded FSUB result or alter the arithmetic helper.

Only the D8 FSUBR admission opts into the larger gap. `DC` reverse and `DE`
FSUBRP callers keep their previous gap-64 limit. The existing twelve masked
PC24/53/64 FCWs, selector and status gates, Valid tags, canonical nonzero
Normal raw operands, and Normal result requirement remain in force. On
success, old physical TOP receives the result, PE is sticky, C1 comes from the
helper's pre-carry `incremented` diagnostic, FIP receives old RIP, and RIP
advances by two. TOP/tags/source payloads and unrelated state are preserved.
The register form performs no data access. Alias cancellation, overflow and
unsupported outcomes are atomic. JIT/native and physical x86 acceptance are
not claimed.

## Verification

- `cargo test -p od-isa --offline fsubr_st0_sti`: 10 focused tests passed.
- `cargo test -p od-runner --offline x87_fsubr_large_gap_literal`: the literal
  fixture test passed; it carries actual `ST(1)=2^-65`, checks the rounded
  negative predecessor at PC53 round-up, and verifies JIT denial.
- `cargo test -q -p od-isa -p od-testkit -p od-runner --offline`: od-isa 1,879,
  od-testkit 455, runner unit 124, runner fixture 513 passed.
- `cargo check --workspace --all-targets --offline` passed.
- `cargo fmt --all -- --check` and `git diff --check` passed.
- `graphify update .` refreshed the graph to 11,753 nodes, 25,313 edges and
  372 communities. A post-refresh query surfaced the new FSUBR oracle,
  refusal test and CPU contract nodes.
- `python3 tools/check_layout.py` reports the isolated `.git` pointer as
  unmapped and two pre-existing missing oracle-manifest links from the shared
  Stage 2 report. No project path was added outside existing directories.

The new raw integer oracle covers gaps 64, 65, 66 and 32765, both sign choices,
both operand orders, all twelve FCWs, every TOP and non-alias source index,
and PE/C1 state. Separate checks cover alias cancellation, overflow rollback,
DC/DE gap-65 refusal, and no data accesses. The rounding oracle uses direct
integer magnitude thresholds rather than production floating-point helpers.

The graph was absent at the start of this isolated checkout, so no pre-change
query was available; the AST refresh and post-refresh query completed. The
layout findings are pre-existing worktree/report limitations. Independent
review is pending. No physical x86, JIT, or native execution is claimed.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dj-large-gap-fsubr-st0-sti-d8-e8-ef),
[Stage 2 plan](../plans/STAGE_2.md#s2-30dj-large-gap-fsubr-st0-sti-admission),
[shared report](STAGE_2_REPORT.md#s2-30dj--large-gap-fsubr-st0-sti),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dj-large-gap-fsubr-fixture-coverage),
and [ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
