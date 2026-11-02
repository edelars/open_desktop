# S2-30cr — bounded `FSUBR ST(0), ST(i)` precision

The Long64 interpreter admits only exact bare `D8 E8..EF`. The operation
computes `ST(i) - ST(0)` with direct FCW-controlled precision-24/53/64 rounding
for all four rounding modes. It flips the sign bit of a validated local ST(0)
copy and adds that value to ST(i) using the existing S2-29ae addition precision
helper. No arithmetic-core API or guest source mutation was added.

Admission requires present x87 state, absent FCS/FDS selectors, one of the
twelve all-exception-masked FCWs, `FSW & 0x80df == 0`, Valid tags, and canonical
nonzero Normal binary80 operands of either sign. The normalized exponent gap
must be at most 64 before the helper is called, independent of the helper's
wider S2-29af domain. Only finite Normal results are accepted. The `i=0` alias
cancels to zero and remains atomically Unsupported; zero, subnormal, overflow,
special and malformed cases also reject.

On success, physical TOP receives the rounded result. PE becomes old PE OR
`inexact`, C1 is replaced from the helper's pre-carry `incremented` diagnostic,
and C0/C2/C3 are retained by local policy. TOP, tags, source bytes, FCW/FDP/FOP
and unrelated state are preserved; FIP receives old RIP, RIP advances by two,
and the instruction retires once. Register execution performs no data access.
All refusals, fetch faults, STOP and zero-budget boundaries are atomic.
`DC` reverse, `DE` pop, FSUB direction, JIT/native behavior and physical x86
acceptance remain outside this slice.

Coverage includes every TOP/index pair, all four operand sign combinations,
all twelve FCWs, p53 midpoint neighbors, directed rounding for both result
signs, exponent gaps 1/63/64 and atomic 65+, alias/cancellation,
subnormal/overflow/zero/special/malformed operand refusals, FSW/FCW/selectors/
tags, preserved sources and register canaries, and fetch/STOP/budget/no-data-
access behavior.

Validation passed: focused `cargo test -p od-isa --offline fsubr_st0_sti`
(8 tests), full `cargo test -p od-isa --offline` (1,788 tests), and
`cargo test -p od-testkit -p od-runner --offline` (454 testkit, 112 runner
unit, 489 runner fixture tests). `cargo check --workspace --all-targets
--offline`, `cargo fmt --all -- --check`, `git diff --check`, and Graphify AST
refresh passed (11,201 nodes, 24,050 edges, 342 communities). Layout validation
reproduced the existing worktree `.git` pointer and two missing historical
oracle-manifest findings tracked by ISSUE-023. Independent Sol/high review is
pending; the Stage 2 plan remains unchecked until review completes.

Semantic reference: Intel SDM Rev. 093 Vol. 2A `FSUBR/FSUBRP/FISUBR`,
pp. 3-397–3-399 ([official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515)); Vol. 1 §8.1.5.2/Table 8-2, §8.1.5.3 and §8.5.6 p. 8-30.

The bounded local contract is recorded in the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cr-bounded-fsubr-st0-sti-d8-e8-ef), [Stage 2 plan](../plans/STAGE_2.md#s2-30cr-bare-fsubr-st0-sti-with-bounded-fcw-precision), and [shared Stage 2 report](STAGE_2_REPORT.md#s2-30cr--bounded-fsubr-st0-sti). No physical x86, native or JIT validation is claimed.
