# S2-30dn — large-gap `FSUBRP ST(i), ST(0)`

Implemented large-gap admission only for exact bare Long64 `DE E0..E7`. The
existing S2-30dc preflight and handler now enable the existing S2-29af-backed
large-gap path. All S2-30dc environment, selector, FCW, FSW, tag, canonical
Normal operand, and Normal-result gates remain in force; normalized gaps up to
64 use the same arithmetic path and retain their prior behavior. There is no
exact fallback. `DE E8..EF` FSUBP, DC forms, D8 forms, JIT and native execution
remain outside this change.

The result remains old `ST(0)-ST(i)`, written to old physical `(TOP+i)&7`
before old TOP is marked Empty and TOP increments. The popped payload is
preserved. PE is sticky OR `inexact`; C1 comes from `incremented`; other
S2-30dc state policy is retained. FIP receives old RIP, RIP advances by two,
and the instruction retires once without data access.

The ISA acceptance matrix has 86,016 independently calculated signed-integer
cases: gaps 64/65/66/32765 × four sign pairs × both magnitude orders × twelve
FCWs × all eight TOPs × every nonalias index × both PE and C1 histories. It
compares complete state, destination-before-pop payload behavior, and the
absence of data callbacks. Separate vectors cover PC24 and PC53 midpoint or
directed rounding. Atomic refusal coverage includes alias cancellation,
malformed and non-Normal domains, unsupported helper results, fetch faults,
STOP, zero budget, and missing x87 state.

The handwritten schema-v2 fixture
`s2_30dn_fsubrp_gap65_pop.case.json` contains actual operands 1.0 and `2^-65`
with bare `DE E3`. Its runner check validates the complete result state,
unchanged code bytes, exception observation, one retirement, and JIT denial.
This is local interpreter evidence only; no physical x86 acceptance is
claimed.

Validation passed: focused `od-isa` `fsub` 55/55; full `od-isa` 1,893/1,893;
`od-testkit` 455/455; and `od-runner` 125 unit + 519 fixture tests. Formatting,
workspace all-target offline check, and `git diff --check` passed. Graphify
refreshed 11,857 nodes, 25,558 edges, and 371 communities. Layout validation
still reports the pre-existing `.git` worktree pointer and two missing
historical oracle manifests recorded in ISSUE-023. Independent Sol/high
implementation review is pending. No physical x86 check was run.

Normative source: Intel SDM Rev. 093 Vol. 2A `FSUBR/FSUBRP/FISUBR`,
pp. 3-397–3-399
([official Vol. 2A PDF](https://cdrdv2.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515));
Vol. 1 §§8.1.5.1–.3, Table 8-2 and §8.5.6
([official Vol. 1 PDF](https://cdrdv2.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).

See the [plan](../plans/STAGE_2.md#s2-30dn-large-gap-fsubrp-sti-st0-admission),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dn-large-gap-fsubrp-sti-st0-de-e0e7),
[shared Stage 2 report](STAGE_2_REPORT.md#s2-30dn--large-gap-fsubrp-sti-st0),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dn-large-gap-fsubrp-fixture-coverage).
