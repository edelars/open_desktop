# S2-30dm — large-gap `FSUBP ST(i), ST(0)`

Bare Long64 `DE E8..EF` now admits normalized Normal-to-Normal exponent gaps
65 through 32765 through the existing S2-29af direct-addition precision
helper. Gap≤64 uses the same validated arithmetic and preserves the S2-30db
result behavior. The old S2-30j exact p64 fallback remains available only
within its existing gap≤64 domain; a large-gap helper refusal cannot fall
through to the exact path.

The profile retains S2-30db's twelve all-masked PC24/53/64 FCWs, FSW and
selector gates, Valid tags, canonical nonzero Normal operands and Normal-result
restriction. It computes old `ST(i)-ST(0)` after validating both original
operands. On success it writes old physical `(TOP+i)&7`, empties the old TOP
tag, then increments TOP. PE remains sticky and C1 is replaced from the
helper's pre-carry `incremented` diagnostic. `i=0` cancellation and unsupported
arithmetic/range outcomes refuse atomically. No data access occurs. The
`DE E0..E7` FSUBRP, `DC E8..EF` FSUB and `D8` neighbors are unchanged.

## Coverage

- An independent integer oracle covers 86,016 full-state cases across gaps
  64/65/66/32765, all twelve FCWs, both operand placements, four sign pairs,
  all TOP/index combinations and prior PE/C1 histories. It independently
  computes the power-of-two difference and rounds on the FCW precision grid.
- Existing alias, invalid-input and execution-boundary tests cover atomic
  refusal; a new explicit large-gap helper-refusal test verifies the exact
  fallback cannot commit a large-gap input. The oracle compares complete
  machine state, including destination placement before pop, tags, TOP, PE/C1
  and unrelated x87/GPR/RFLAGS state, and confirms no data access.
- The literal
  [`DE E9` gap-65 fixture](../../tests/fixtures/x86/stage2/s2_30dm_fsubp_sti_st0_gap65_down.case.json)
  uses actual `2^-65` bytes, checks the full CPU snapshot, exception
  observation and unchanged memory, and confirms JIT denial.
- No physical x86 acceptance is claimed.

## Validation

Validation passed:

- Focused FSUBP ISA filter: 9 passed, including exhaustive bare `DE E8..EF`
  decoder checks. The independent gap oracle and exact-fallback refusal test
  also passed.
- `cargo test -q -p od-isa -p od-testkit -p od-runner --offline -- --test-threads=1`:
  od-isa 1,889; od-testkit 455; runner 125 unit and 517 fixture tests passed.
- `cargo check --workspace --all-targets --offline`,
  `cargo fmt --all -- --check` and `git diff --check` passed.
- `graphify update .` refreshed the code graph to 11,827 nodes, 25,484 edges
  and 382 communities. Community labels were not refreshed because no Graphify
  LLM API key is configured in the worktree.
- `python3 tools/check_layout.py` reported only the isolated worktree `.git`
  marker and two unavailable historical oracle manifests linked from the
  shared report (the existing ISSUE-023 caveat); no new local path violations
  were reported.

No physical x86 acceptance was performed. Independent Sol/high review is
pending.

Sources: Intel SDM Rev. 093 Vol. 2A `FSUB/FSUBP/FISUB`, pp. 3-394–3-396;
Vol. 1 §§8.1.5.1–.3, Table 8-2 and §8.5.6, pp. 8-7–8-8 and 8-30. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dm-large-gap-fsubp-sti-st0-de-e8i),
[plan](../plans/STAGE_2.md#s2-30dm-large-gap-fsubp-sti-st0-admission),
[shared report](STAGE_2_REPORT.md#s2-30dm--large-gap-fsubp-sti-st0),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dm-large-gap-fsubp-fixture-coverage),
and [ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
