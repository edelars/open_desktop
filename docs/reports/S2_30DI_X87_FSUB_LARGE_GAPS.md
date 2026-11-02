# S2-30di — large-gap `FSUB ST(0), ST(i)` precision

The Long64 interpreter now admits only bare `D8 E0..E7` FSUB with canonical
nonzero Normal operands whose normalized exponent gap is through 32765. The
existing S2-29af direct-addition precision helper supplies large-gap rounding.
The original RHS is validated before its sign bit is flipped in a local raw
copy. The shared FSUB precision preflight takes an explicit large-gap option;
only the D8 forward instruction enables it, while DC FSUBR and DE FSUBRP retain
the gap-64 limit.

The existing twelve all-exception-masked PC24/53/64 FCWs, selectors, required
FSW mask, Valid tags, Normal result domain, sticky PE, pre-carry C1 diagnostic,
physical TOP destination and unrelated state behavior are preserved. Exact
cancellation, including alias `i=0`, overflow, and all other unsupported
results reject before mutation. The register instruction performs no data
access. JIT/native admission and physical x86 acceptance remain closed.

## Verification

The independent raw-integer power-magnitude oracle covers gaps 64, 65, 66 and
32765; all twelve FCWs; both signs and operand orders; every TOP and non-alias
source index; full state, PE/C1 and memory-access checks. A separate refusal
matrix verifies alias cancellation and atomic gap-65 rejection for DC FSUBR and
DE FSUBRP. The schema-v2 runner fixture contains the literal RHS bytes
`0000000000000080be3f` (`2^-65`) and checks rounded state plus JIT denial.

Passed checks:

- Focused oracle and refusal tests, plus the literal runner test.
- `cargo test -q -p od-isa -p od-testkit -p od-runner --offline`: od-isa 1,873,
  od-testkit 455, runner unit 122 and runner fixture 512.
- `cargo check --workspace --all-targets --offline`.
- `cargo fmt --all -- --check` and `git diff --check`.
- `graphify update .`: 11,715 nodes, 25,226 edges, 371 communities.
- Local Markdown target check for changed docs passed after this report is added.

`python3 tools/check_layout.py` still reports the isolated worktree's `.git`
marker as unmapped and two pre-existing links from the shared Stage 2 report to
`work/` manifests not present in this checkout. A targeted check of all eight
new local links in the plan, contract, shared/standalone reports and fixture
notes passed. No project path was added outside existing directories. These
are environment and historical-report limitations, not changes to the
implementation.

Independent Sol/high review remains pending in the retained worktree for parent
review. No physical x86 execution or native/JIT support is claimed.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30di-large-gap-fsub-st0-sti-d8-e0-e7),
[Stage 2 plan](../plans/STAGE_2.md#s2-30di-large-gap-fsub-st0-sti-admission),
[shared report](STAGE_2_REPORT.md#s2-30di--large-gap-fsub-st0-sti),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30di-large-gap-fsub-fixture-coverage),
and [ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
