# S2-30cz — bounded `FADDP ST(i), ST(0)` precision profile

Only bare Long64 `DE C0..C7` is admitted. The interpreter reuses the S2-30ct
bounded binary80 addition helper with destination `ST(i)` and source `ST(0)`.
Preflight requires one of twelve all-masked PC24/53/64 FCWs, absent x87
selectors, `FSW & 0x80df == 0`, Valid canonical nonzero Normal operands,
normalized exponent gap≤64 and finite Normal output. Both input values and the
result are captured and checked before mutation.

Success writes to old physical `(TOP+i)&7`, marks old physical TOP Empty, then
increments TOP modulo eight without rotating the register array. This preserves
the `i=0` payload beneath its Empty tag and makes the `i=1` result new ST(0).
PE is sticky from `inexact`; C1 comes from `incremented`. FIP receives old RIP,
RIP advances by two, retirement occurs once, and no data-memory access occurs.
Unsupported values, environments, gaps, outputs and execution boundaries leave
state unchanged. JIT/native admission, guest exception delivery and physical
x86 acceptance remain outside this local profile.

## Validation

- Focused `cargo test -p od-isa faddp_sti_st0 --offline`: 7 tests passed,
  including a 3,072-case TOP/index/FCW/sign matrix, literal PC24 midpoint
  rounding, gap-64 PE, gap-65 rollback, i=0 overflow atomicity and execution
  boundaries.
- `cargo test -p od-runner x87_faddp_fixture_runner --offline`: 2 tests passed;
  fixtures compare the full x87 snapshot for all 64 TOP/index pairs and a
  rounded PE/pop/TOP-wrap case. JIT compilation remains denied.
- The testkit schema admits all eight exact encodings. The Linux-only native
  admission test source enumerates DE C0..C7; it was not executable on this
  macOS host. Full workspace tests passed: od-isa 1,834; od-testkit 455; runner
  119 unit + 496 fixture; x86-runner 8 library + 4 binary tests.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `python3 tools/check_layout.py`: reports the worktree `.git` marker and two
  historical oracle manifest links already absent in this checkout; all new
  report links are added here.
- `graphify update .`: 11,451 nodes, 24,663 edges and 361 communities.
- Independent `gpt-6.1-sol` high review: clean; no findings.

Semantic basis: Intel SDM Rev. 093 Vol. 2A `FADD/FADDP/FIADD`, pp. 3-311–3-313,
and Vol. 1 §§8.1.5.1–.3 and §8.5.6. This slice does not claim general FADDP,
special values, stack faults, exception delivery, CR0 behavior, JIT/native
execution or physical x86 acceptance. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cz-bounded-faddp-sti-st0-de-c0c7),
[plan](../plans/STAGE_2.md#s2-30cz-bare-bounded-precision-faddp-sti-st0),
[shared report](STAGE_2_REPORT.md#s2-30cz--bounded-faddp-sti-st0),
[interpreter methodology](../testing/INTERPRETER.md#local-faddp-precision-profile--s2-30cz),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30cz-x87-schema-and-fixture-coverage).
