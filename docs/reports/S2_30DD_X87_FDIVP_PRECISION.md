# S2-30dd — bounded `FDIVP ST(i), ST(0)` precision

Implemented FCW-selected precision for exact bare Long64 `DE F8..FF`. The
operation computes old `ST(i) / ST(0)` by reusing
`bounded_x87_fdivr_st0_sti_result`; no arithmetic helper or exact fallback was
added. Admission uses the existing bounded profile: present x87 state, absent
FCS/FDS, one of twelve all-exception-masked PC24/53/64 FCWs,
`FSW & 0x80df == 0`, Valid operand tags, canonical nonzero Normal binary80
operands, and a finite Normal result. The existing p64 helper domain is kept,
including unequal exponents and gap 65. The `i=0` alias produces exact `+1`.

Success writes the quotient to old physical `(TOP+i)&7`, then empties old TOP
and increments TOP modulo eight. For `i=0`, `+1` is written before that same
register is emptied. PE remains sticky and accumulates `inexact`; C1 is
replaced from the helper's `incremented` diagnostic. The local deterministic
policy preserves C0/C2/C3, selectors, unrelated payload and unaffected state;
FIP receives old RIP, RIP advances by two, and the instruction retires once.
Unsupported profile inputs, fetch failures, STOP and zero-budget boundaries
do not partially commit. JIT/native execution, general x87 exception delivery
and physical x86 acceptance remain outside this slice.

Coverage includes 12,288 complete-state ISA cases crossing all twelve FCWs,
all TOP and index values, four sign pairs and four PE/C1 histories. Gap-65
cases cover every FCW, TOP and nonalias destination. The runner matrix covers
all FCWs and a handwritten schema-v2 PC24 round-toward-negative `-1/3` fixture
checks literal result bits, complete state, unchanged memory and JIT denial.
The schema-v2 validator now admits valid inexact FDIVP results while retaining
atomic rejection of malformed/unsupported profile inputs. Neighbor encodings
and runner/JIT denial remain checked by the focused and full component suites.

Validation passed: focused ISA `fdivp` 4/4; full `od-isa` 1,851/1,851;
`od-testkit` 455/455; focused runner FDIVP unit 1/1 and fixture 1/1; full
runner unit 119/119 and serialized `fixture_v2` 504/504 after the review fix.
One earlier post-fix serialized run had 503/504 because the BT CLI test
transiently received ENOENT from `Command::output`; that test passed alone 1/1,
and a clean full serialized rerun passed all 504. `cargo check --workspace --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check` passed.
The isolated `tools/check_layout.py` run reported the unmapped `.git` marker
and two missing historical oracle manifests already tracked by ISSUE-023; the
parent workspace layout check is not represented by this isolated result.
Graphify AST refresh completed at 11,516 nodes, 24,928 edges and 347
communities; semantic documentation extraction was not run. Independent
Sol/high review is clean at `58b37d1`; confirmed schema and documentation
findings were corrected. No physical x86 acceptance is claimed.

Normative source: Intel SDM Rev. 093 Vol. 2A `FDIV/FIDIV—Divide`,
pp. 3-333–3-335
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451));
Vol. 1 §§8.1.5.1–.3, Table 8-2 and §8.5.6
([official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [plan](../plans/STAGE_2.md#s2-30dd-bare-fdivp-sti-st0-with-bounded-fcw-precision),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dd-bounded-fdivp-sti-st0-de-f8ff),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dd-bounded-fdivp-fixture-coverage).
