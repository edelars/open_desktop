# S2-30dg — large-gap reverse x87 FADD

## Scope

Extended only exact bare Long64 `DC C0..C7` (`FADD ST(i),ST(0)`) to admit
normalized Normal-to-Normal exponent gaps 65 through 32765. It reuses the
existing S2-29af direct-precision addition bridge. The operation-specific
large-gap wrapper is used only for non-popping `Operation::FaddStSt0`;
`DE C0..C7` FADDP continues through the gap≤64 wrapper. The arithmetic helper,
other `DC` operations, `D8` FADD and subtraction families are unchanged.

The existing environment, selector, status, Valid-tag, canonical nonzero
Normal operand/result checks remain in preflight. Successful writes target
old physical `(TOP+i)&7`; TOP, tags, the source register, unrelated state and
condition codes remain preserved, with sticky PE, helper-derived C1, FIP/RIP
and one retirement. Refusals stay atomic.

## Independent coverage

An independent integer oracle computes the raw rounded binary80 result for
power-of-two operands without calling the production addition helper. It
checks 288 combinations: gaps 65, 66 and 32765; high operand in either
physical position; positive and negative dominant operands; same-sign and
opposite-sign tails; and all twelve masked PC24/53/64 FCWs. It checks result
bits and full CPU state, including preserved prior status fields and no data
access. Adjacent coverage verifies gap 64 behavior, maximum-range cases,
overflow/cancellation refusal and atomicity; `DE C1` still refuses gap 65.

The literal fixture
[`s2_30dg_fadd_sti_st0_gap65_up.case.json`](../../tests/fixtures/x86/stage2/s2_30dg_fadd_sti_st0_gap65_up.case.json)
contains exact `DC C1` bytes and actual `2^-65` Normal input. The runner
checks the full interpreter snapshot and JIT denial. Native admission has a
fixture-backed denial test in the Linux-only native test module; this
macOS validation environment does not compile that module.

## Validation

- `cargo test -p od-isa --offline`: 1,866 passed.
- `cargo test -p od-testkit --offline --quiet`: 455 passed.
- `cargo test -p od-runner --offline --quiet`: 120 unit and 509 fixture tests passed.
- `cargo test -p od-x86-runner --offline --quiet`: 8 library and 4 binary tests passed.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
  and `git diff --check` passed.
- `python3 tools/check_layout.py` reports the expected isolated-worktree
  `.git` pointer plus two absent historical oracle manifests already noted in
  ISSUE-023. The report link added by this slice is present.
- Physical x86 execution was not performed. Independent Sol/high review is
  pending transfer to the parent task.

Source: Intel SDM Rev. 093 Vol. 2A `FADD/FADDP/FIADD`, including `DC C0+i`
encoding and Operation/FPU Flags Affected, pp. 3-311–3-313; Vol. 1
§8.1.5.1–.3, Table 8-2 and §8.5.6.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ct-bounded-fadd-sti-st0-dc-c0c7),
[plan](../plans/STAGE_2.md#s2-30dg-large-gap-fadd-sti-st0-admission),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dg-large-gap-reverse-fadd-fixture-coverage),
and [shared report](STAGE_2_REPORT.md#s2-30dg--large-gap-reverse-fadd-stist0).
