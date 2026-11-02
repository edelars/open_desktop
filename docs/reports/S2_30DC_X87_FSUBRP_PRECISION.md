# S2-30dc — bounded `FSUBRP ST(i), ST(0)` precision

Implemented direct FCW-selected precision for exact bare Long64 `DE E0..E7`.
The operation computes old `ST(0) - ST(i)` by reusing
`bounded_x87_fsub_st0_sti_result`; it adds no arithmetic helper and no exact
fallback. Admission requires present x87 state, absent FCS/FDS, one of twelve
all-exception-masked PC24/53/64 FCWs, `FSW & 0x80df == 0`, Valid tags,
canonical nonzero Normal binary80 operands, and normalized exponent gap≤64.
The `i=0` alias cancels and remains atomically Unsupported. Only finite Normal
results are accepted, and the existing helper's p64 domain is retained.

Success writes old physical `(TOP+i)&7`, marks old TOP Empty and increments
TOP modulo eight. The popped payload remains intact. PE is sticky and ORed with
`inexact`; C1 is replaced from `incremented`; C0/C2/C3 follow the local
preservation policy. FIP receives old RIP, RIP advances by two and the
instruction retires once without data access. Invalid domains and fetch/STOP/
budget boundaries do not partially commit. JIT/native execution, general x87
exception delivery and physical x86 acceptance remain closed.

A 10,752-case ISA matrix crosses the twelve FCWs, all TOP and nonalias index
placements, four operand sign pairs and PE/C1 histories, comparing complete
post-state and no-data-access behavior. Rollback coverage includes alias,
unsupported FCW/FSW, selectors, tags, malformed/zero operands, result range and
gap 65. A handwritten schema-v2 `DE E2` fixture is a PC24 midpoint and
double-rounding discriminator with complete post-state, exception observation
and unchanged memory. Runner checks also exercise directed negative rounding
and JIT denial. Existing exhaustive DE ModR/M/prefix decoding and the
FSUBP/FSUBR neighbors remain covered.

Validation passed: focused `od-isa` `fsub` filter 43/43; full `od-isa`
1,847/1,847; `od-testkit` 455/455; runner 119 unit + 502 fixture tests; and
`od-x86-runner` 8 library + 4 binary tests. Runner fixtures include the new
full-state midpoint / negative-directed case and adjacent S2-30da/b cases.
Workspace all-target offline check, `cargo fmt --all -- --check`, and
`git diff --check` passed. Graphify AST refresh completed at 11,535 nodes,
24,827 edges and 369 communities. The isolated layout check reports its `.git`
marker and two missing historical oracle manifests already recorded under
ISSUE-023; no new broken links were reported. Independent Sol/high review is clean; the reviewer independently reran the
43-test `fsub` filter. A dedicated FSUBRP odd-65 vector was not added; the
shared helper’s odd-65 rounding is covered by adjacent S2-30db, while this slice
directly covers gap-64 inexact rounding and the p64 exact path. No physical x86
acceptance is claimed.

Normative source: Intel SDM Rev. 093 Vol. 2A `FSUBR/FSUBRP/FISUBR`,
pp. 3-397–3-399
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515));
Vol. 1 §§8.1.5.1–.3, Table 8-2 and §8.5.6, pp. 8-7–8-8 and 8-30
([official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [plan](../plans/STAGE_2.md#s2-30dc-bare-fsubrp-sti-st0-with-bounded-fcw-precision),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dc-bounded-fsubrp-sti-st0-de-e0e7),
[shared report](STAGE_2_REPORT.md#s2-30dc--bounded-fsubrp-sti-st0), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dc-bounded-fsubrp-fixture-coverage).
