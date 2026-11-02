# S2-30cq — bounded `FSUB ST(0), ST(i)` precision

The Long64 interpreter admits only exact bare `D8 E0..E7`. It performs
direct FCW-controlled precision-24/53/64 subtraction for all four rounding
modes by flipping the sign bit of a validated local RHS copy and calling the
existing S2-29ae addition precision helper. No arithmetic-core API or guest
source mutation was added.

Admission requires present x87 state, absent FCS/FDS selectors, one of the
twelve all-exception-masked FCWs, `FSW & 0x80df == 0`, Valid tags, and
canonical nonzero Normal binary80 operands of either sign. The normalized
exponent gap must be at most 64 before the helper is called, independent of its
wider S2-29af domain. Only finite Normal results are accepted. The `i=0`
alias cancels to zero and remains atomically Unsupported; other zero,
subnormal, overflow and malformed cases also reject.

On success, physical TOP receives the rounded result. PE becomes old PE OR
`inexact`, C1 is replaced from the pre-carry `incremented` diagnostic, and
C0/C2/C3 are retained by local policy. TOP, tags, source bytes, FCW/FDP/FOP
and unrelated state are preserved; FIP receives old RIP, RIP advances by two,
and the instruction retires once. Register execution performs no data access.
All refusals, fetch faults, STOP and zero-budget boundaries are atomic.
Reverse `DC`, pop `DE`, FSUBR, JIT/native and physical x86 acceptance remain
outside this slice.

Coverage includes every TOP/index pair and sign combination across all twelve
FCWs, sticky PE/C1 and condition-code preservation, p53 midpoint neighbors,
directed rounding for both result signs, exponent gaps 1/63/64 and atomic
65+, cancellation/alias, subnormal result, overflow, zero and malformed
operand refusals, and fetch/STOP/budget/no-data-access behavior.

Validation passed: focused `cargo test -p od-isa --offline fsub_st0_sti`
(7 tests), full `cargo test -p od-isa --offline` (1,780 tests), and
`cargo test -p od-testkit -p od-runner --offline` (454 testkit, 112 runner
unit, 487 runner fixture tests). `cargo check --workspace --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check` passed.
Graphify AST refresh passed (11,106 nodes, 23,974 edges, 331 communities).
Layout validation reports the known worktree `.git` pointer and two missing
historical oracle manifests tracked by ISSUE-023; no layout changes were made.
Independent Sol/high review completed clean after correcting the refusal-test
baseline and removing a duplicate admission check. This report does not claim
physical x86, native or JIT validation.

After integration with SQRTSS in main, the combined suites passed: full
`od-isa` 1,782, `od-testkit` 454, runner 112 unit + 488 fixture tests, and
workspace all-target offline check. Formatting, layout and Graphify refresh
also passed on the integrated tree.

Semantic reference: Intel SDM Rev. 093 Vol. 2A `FSUB/FSUBP/FISUB`,
pp. 3-394–3-396 ([official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512)); Vol. 1 §8.1.5.2/Table 8-2, §8.1.5.3 and §8.5.6 p. 8-30.
The bounded local contract is recorded in the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cq-bounded-fsub-st0-sti-d8-e0-e7)
and [Stage 2 plan](../plans/STAGE_2.md#s2-30cq-bare-fsub-st0-sti-with-bounded-fcw-precision).
