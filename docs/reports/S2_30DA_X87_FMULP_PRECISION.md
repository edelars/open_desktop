# S2-30da — bounded FCW-precision `FMULP ST(i), ST(0)`

Implemented bounded FCW-precision execution for exact bare Long64 `DE C8..CF`.
FMULP reuses the validated S2-30cu multiplication helper with `ST(i)` as the
destination and `ST(0)` as the source. It requires x87 state with absent FCS/FDS,
one of twelve all-exception-masked PC24/53/64 FCWs, clean required FSW bits,
Valid tags and canonical nonzero Normal binary80 operands. The alias `i=0`
validates one physical TOP operand and squares it. Only finite Normal results
are admitted.

Success writes the rounded product to old physical `(TOP+i)&7` before emptying
old physical TOP, including the alias case. It ORs PE with `inexact`, replaces
C1 with the helper's pre-carry magnitude `incremented` diagnostic, preserves
C0/C2/C3 by the local deterministic policy, advances TOP, stores old RIP in
FIP, advances RIP by two and retires once. Other payloads, tags, FCW/FDP/FOP,
GPRs and RFLAGS are preserved. Unsupported inputs, range failures and execution
boundaries reject atomically. The implementation performs no data access.
Schema-v2 still admits the eight existing encodings; native/JIT and general
x87 exception delivery remain unsupported.

## Coverage and validation

- `cargo test -p od-isa fmulp_sti_st0 --offline`: 3 focused tests passed,
  including all 64 TOP/index pairs across twelve FCWs and both signs.
- `cargo test -p od-isa --offline -q`: 1,838 tests passed.
- Focused runner FMULP fixture test: passed.
- `cargo test -p od-runner --test fixture_v2 s2_30da_fmulp_fixture_executes_and_matches_the_complete_post_state --offline`:
  passed; it compares the full resulting CPU state, exception observation and
  unchanged memory bytes against the static PC64 inexact fixture.
- `cargo test -p od-runner --test fixture_v2 --offline -q`: 498 tests passed,
  including that explicit static-fixture test.
- `cargo test -p od-testkit --offline -q`: 455 tests passed.
- Runner unit tests: 119 passed.
- `cargo fmt --all -- --check`, `git diff --check` and
  `cargo check --workspace --all-targets --offline`: passed.
- `graphify update .`: passed; graph refreshed to 11,480 nodes, 24,716 edges
  and 360 communities.
- `python3 tools/check_layout.py`: reports only the isolated worktree's `.git`
  marker and two unavailable historical oracle manifests tracked by ISSUE-023;
  it reports no broken local links for this change.

Independent Sol/high review found stale documentation anchors and that the
static fixture was not connected to the `fixture_v2` suite. The follow-up
corrected the links and added an explicit full-state fixture assertion; review
of that follow-up is pending. No physical x86 or native x87 acceptance is
claimed. Sources: Intel SDM Rev. 093 Vol. 2A
`FMUL/FMULP/FIMUL`, pp. 3-360–3-362; Vol. 1 §§8.1.5.1–.3, Table 8-2 and
§8.5.6, pp. 8-7–8-8 and 8-30. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30da-bounded-fcw-precision-fmulp-sti-st0-de-c8i),
[plan](../plans/STAGE_2.md#s2-30da-bare-fmulp-sti-st0-with-bounded-fcw-precision),
[shared report](STAGE_2_REPORT.md#s2-30da--bounded-fmulp-sti-st0), and
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
