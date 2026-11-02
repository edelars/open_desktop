# S2-30cy: bounded reverse x87 `FDIVR ST(i), ST(0)`

## Scope

The interpreter accepts exact bare Long64 `DC F0..F7`. It computes old
`ST(0)/ST(i)` at the selected x87 FCW precision and writes the result into old
physical `(TOP+i)&7`, without popping. It directly reuses
`bounded_x87_fdiv_st0_sti_result`; division has no exponent-gap cutoff. The
`i=0` alias validates only the one used register and produces exact +1.

Admission requires present x87 state, absent FCS/FDS, `FSW & 0x80df == 0`, one
of twelve all-exception-masked PC24/53/64 control words, Valid tags for used
operands and canonical nonzero Normal binary80 values. Only finite Normal
quotients are admitted. Success accumulates PE, replaces C1 from the rounding
increment diagnostic, preserves C0/C2/C3 and unrelated state, records FIP,
advances RIP by two and retires once. Invalid environments, tags, operands and
results refuse atomically.

The previous exact p64 result domain is checked against
`exact_fdiv_result(state, 0, i)`, including differing exponents. The testkit's
former rejected `DC F1` 1/3 operation now asserts rounded success, literal p64
result, PE/C1, FIP/RIP, one retirement and full-state equality. The shared
rounding matrix covers every FCW × TOP × index × operand sign × prior PE/C1
state, with literal PC24/53/64 1/3 values; a separate case checks an exponent
gap above 64 at all twelve FCWs. Existing invalid profile cases continue to
check atomic refusal. Native/JIT/backend admission and physical x86 acceptance
are not claimed.

## Validation

- Focused `od-isa` FDIVR tests: 6/6 passed.
- Full `od-isa`: 1,829 passed.
- Focused testkit FDIVR runner: 1/1 passed.
- Full `od-testkit`: 455 passed.
- Workspace all-target offline check, formatting check and `git diff --check`
  passed.
- Isolated layout check reports the worktree `.git` pointer and the two
  existing missing ISSUE-023 oracle manifests; main-worktree layout
  verification remains for integration.
- Graphify query was run before implementation in main. Isolated Graphify AST
  refresh passed: 11,424 nodes, 24,603 edges and 359 communities.
- Independent Sol/high review is clean: ISA 6/6, neighboring `fdiv` 22/22, testkit runner 1/1.

Normative references: Intel SDM Rev. 093 Vol. 2A `FDIVR/FDIVRP/FIDIVR`, pp. 3-336–3-338;
Vol. 1 §8.1.5.2/Table 8-2, §8.1.5.3 and §8.5.6. See the [Stage 2 plan](../plans/STAGE_2.md#s2-30cy-bare-fdivr-sti-st0-with-bounded-fcw-precision),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cy-bounded-fdivr-sti-st0-dc-f0-f7),
[shared report](STAGE_2_REPORT.md#s2-30cy--bounded-fdivr-sti-st0), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30cy-x87-reverse-division-coverage).
