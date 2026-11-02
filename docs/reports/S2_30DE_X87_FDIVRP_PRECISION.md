# S2-30de bounded x87 FDIVRP precision

## Scope and contract

S2-30de admits only bare Long64 `DE F0..F7` (`FDIVRP ST(i), ST(0)`). The
implementation reuses `bounded_x87_fdiv_st0_sti_result` for the old
`ST(0)/ST(i)` operands. It admits the twelve all-exception-masked PC24/53/64
FCWs with all four rounding modes, valid tags, canonical nonzero Normal
binary80 inputs, and finite Normal results in the existing p64 helper domain.
It adds no arithmetic helper, exact fallback, or exponent-gap cutoff.

Success writes the quotient into old physical `(TOP+i)&7` before emptying old
TOP and incrementing TOP. The alias `i=0` writes exact `+1` to old TOP before
marking that physical slot Empty. PE is sticky from prior PE or inexactness;
C1 reports the rounding helper's incremented diagnostic. Other state follows
the local deterministic preservation policy. Invalid controls, x87 status or
selectors, tags, operands, and result range refuse atomically. Fetch, STOP, and
zero-budget boundaries remain atomic. JIT/native execution and physical x86
acceptance remain closed.

## Coverage

- Direct ISA matrix: 12,288 complete-state combinations of FCW, TOP, index,
  signs, and initial PE/C1 history, including `+1/3`, `-1/3`, exact values,
  and the alias.
- Gap-65 division coverage across admitted FCWs, TOPs, and indices, plus
  existing bounds and refusal cases.
- Testkit runner coverage checks exact reverse quotients across TOP/index
  destinations and atomic overflow refusal. The PC24 static CLI fixture covers
  reverse rounding. Schema preflight-refusal allowance is scoped to `DE F1`
  with `stop_rip: null` for result-range refusals such as overflow/underflow;
  selector refusals are covered by direct ISA tests because the fixture schema
  does not admit selectors with this encoding. Valid inexact quotients are
  accepted.
- Static runner fixture: handwritten PC24 `DE F2`, old `ST(0)=-1` divided by
  old `ST(2)=3`, round toward negative, full post-pop state, unchanged memory,
  and JIT denial.
- Neighboring encodings and execution rollback/fetch/STOP/budget boundaries
  are covered by targeted tests.

## Validation

Validation passed:

- `cargo test -p od-isa --offline`: 1,856 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline --bin od-runner`: 119 passed.
- `cargo test -p od-runner --offline --test fixture_v2 -- --test-threads=1`:
  506 passed.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all --
  --check`, and `git diff --check` passed.
- `graphify update .`: 11,591 nodes, 24,949 edges, 361 communities.
- `python3 tools/check_layout.py` reported the isolated-worktree `.git`
  marker and the two pre-existing ISSUE-023 oracle manifest links absent from
  this checkout; no slice path violations were reported.

Independent Sol/high review is clean. The reviewer also passed fdiv 26/26,
testkit FDIVRP 1/1, focused runner 1/1, and checked CLI overflow/underflow
refusals. No physical x86 acceptance is claimed.
