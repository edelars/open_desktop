# S2-30dh — large-gap FADDP ST(i), ST(0)

Bare Long64 `DE C0..C7` FADDP now admits normalized Normal-to-Normal exponent
gaps 65 through 32765. It routes through the existing S2-29af precision helper;
the helper and other instruction families are unchanged. Gap≤64 behavior is
preserved.

Admission still requires present x87 state, absent instruction/data selectors,
one of twelve all-exception-masked PC24/53/64 FCWs, `FSW & 0x80df == 0`,
Valid-tagged canonical nonzero Normal operands and a finite Normal result.
Operands are captured before calculation. On success, write the rounded value
to old physical `(TOP+i)&7`, empty the old TOP tag, then increment TOP modulo
eight without rotating the register array. `i=0` retains its written payload
beneath the Empty tag. PE is sticky from `inexact`; C1 is replaced by the
pre-carry `incremented` diagnostic. Overflow and unsupported state remain
atomic refusals.

The independent raw-integer oracle rounds power-of-two inputs directly on the
requested precision grid, without calling the production arithmetic helper.
It covers 1,152 combinations across gaps 65/66/32765, twelve FCWs, signs,
operand placement, representative TOP/index wrap cases, and the lowest
possible Normal result exponent for the large-gap domain. Additional cases cover
gap 64 and gap 65 boundaries, i=0 alias/pop ordering, overflow rollback, DC
large-gap FADD acceptance, and FSUBP large-gap refusal. The literal
[`DE C1` fixture](../../tests/fixtures/x86/stage2/s2_30dh_faddp_sti_st0_gap65_up.case.json)
uses actual `2^-65` input bytes and checks complete state plus JIT denial.

## Validation

- `cargo test -p od-isa faddp_sti_st0 --offline`: 8 passed.
- Raw-integer boundary/oracle case: 1 passed; the fixture runner case: 1 passed.
- `cargo test -p od-isa --offline`: 1,868 passed; `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline`: 120 unit + 511 fixture tests passed.
- `cargo test -p od-x86-runner --offline`: 8 library + 4 binary tests passed.
  The new native-admission denial source test is Linux-only and did not run on
  this macOS host; no physical x86 acceptance is claimed.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check` and `git diff --check`: passed.
- `python3 tools/check_layout.py` reports the isolated `.git` marker and two
  historical oracle manifests already unavailable in the checkout; no new
  layout violations. All seven new S2-30dh local documentation references
  resolve to existing files and headings.
- `graphify update .`: 11,637 nodes, 25,204 edges, 338 communities.
- Independent Sol/high review completed clean; the final oracle includes the
  minimum Normal-result boundary vector.

## Main integration

Integrated with S2-37dy. Combined main validation passed: od-isa 1,871,
od-testkit 455, runner 121 unit + 512 fixture, and od-x86-runner 8 library + 4
binary tests. Workspace all-target offline check, formatting, layout, and diff
check passed. Main Graphify refresh: 11,660 nodes, 25,258 edges, 368
communities. The added native-denial regression is in a Linux-only module and
did not execute on macOS. No physical x86 acceptance is claimed.

No native x87/JIT execution, guest exception delivery or physical x86
acceptance is claimed. Scope and links: [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dh-large-gap-faddp-sti-st0-de-c0c7),
[Stage 2 plan](../plans/STAGE_2.md#s2-30dh-large-gap-faddp-sti-st0-admission),
[shared Stage 2 report](STAGE_2_REPORT.md#s2-30dh--large-gap-faddp-sti-st0),
[fixture coverage](../../tests/fixtures/x86/stage2/README.md#s2-30dh-large-gap-faddp-fixture-coverage).
