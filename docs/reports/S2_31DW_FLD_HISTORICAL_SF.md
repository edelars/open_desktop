# S2-31dw — historical SF with IE through bounded m32/m64 FLD

The exact bare Long64 `D9 00` (`FLD m32fp [RAX]`) and `DD 00`
(`FLD m64fp [RAX]`) gates now admit historical stack-fault status only when
historical IE is already set. The operation-local reject predicate is
`FSW & 0x8080 != 0 || (SF != 0 && IE == 0)`. SF-only, ES and B therefore remain
pre-read rejects, including when the memory operand is an SNaN that would set
current IE. The schema-v2 no-stop, budget-one Unsupported predicate uses the
same gate.

All current conversion and status behavior remains unchanged. The test matrix
covers 114,688 accepted executions: both source widths; positive and negative
zero, minimum-normal, interior-Normal, infinity, QNaN, SNaN and minimum-subnormal
inputs; all 32 historical DE/ZE/OE/UE/PE combinations with SF+IE; the four
admitted FCWs; all TOP values; and both prior C1 values. It compares the
complete resulting machine state and verifies payloads, tags, metadata, exactly
one source read, and unchanged memory canaries. Separate negative cases prove
SF-only, ES and B reject atomically.
The FLD→FNSTSW→FNCLEX composition observes SF+IE, then verifies clearing.
Schema-v2 coverage prevents an exact bare budget-one/no-stop Unsupported case
from being justified by the now-admitted SF+IE history.

Validation: the focused `od-isa` FLD suite passed 8/8 and the focused
`od-testkit` schema test passed 1/1; `od-runner`'s `fixture_v2` integration
suite passed 522/522. `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, `git diff --check`, and `graphify update .`
passed. `python3 tools/check_layout.py` remains nonzero on
the existing unmapped root `.git` worktree marker and two broken oracle-manifest
links in `STAGE_2_REPORT.md`; this change did not alter those paths.

Graphify refreshed to 11,912 nodes, 25,692 edges and 382 communities.

This is local interpreter and schema evidence. It does not add an SF producer,
general exception delivery, JIT/native admission, or physical x86 acceptance;
ISSUE-019 remains open. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dw-historical-sf-with-ie-for-exact-binary32binary64-fld), [plan](../plans/STAGE_2.md#s2-31dw-admit-historical-sfie-through-exact-binary32binary64-fld), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31dw-historical-sf-with-ie-in-bounded-m32m64-fld).
