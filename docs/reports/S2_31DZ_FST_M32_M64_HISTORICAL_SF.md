# S2-31dz — historical SF with pre-existing IE for bounded m32/m64 stores

## Реализация

Only the existing exact bare `D9 10`, `D9 18`, `DD 10`, and `DD 18` bounded
`FST/FSTP m32fp/m64fp` paths admit historical SF, and only when IE is already
set. The m32/m64 conversion preflight rejects ES and B, and rejects SF when IE
is clear before inspecting or converting ST(0). A current SNaN cannot make an
SF-only status admissible. Other x87 store paths, including raw m80, retain
their existing predicates.

The successful write remains the commit boundary: historical SF and IE persist,
new conversion flags/C1 and any FSTP pop become visible only after the
all-or-error store succeeds. Tests cover 73,728 combinations: four forms, four
FCWs, eight TOP values, all 32 prior DE/ZE/OE/UE/PE histories, both C1 states
and nine representative source/tag vectors, including QNaN and canonical
subnormal. The matrix compares the full resulting CPU state. Separate checks
cover SF-only with SNaN before conversion, SF+IE malformed-source/tag/control
refusals, write/range rollback, repeated store → FNSTSW → FNCLEX → retry, and
schema-v2 Unsupported classification. Four literal success fixtures and one
literal Unsupported refusal fixture are checked. Each success fixture is also
run through JIT and native selectors and must be denied.

## Проверки

- `cargo test -p od-isa --offline --quiet`: 1,920 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-isa m32_m64_stores_ --offline`: 8 passed.
- `cargo test -p od-runner local_x87_m32_m64_stores_ --offline`: 3 fixture tests passed.
- `cargo test -p od-runner x87_store_schema_treats_sf_as_admitted_only_with_historical_ie --offline`: 1 passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `graphify update .`: 12,015 nodes, 25,946 edges, 381 communities; labels were not refreshed because no LLM API key is configured.
- `python3 tools/check_layout.py`: reported the isolated worktree `.git` pointer and two missing historical Stage 1/2 oracle manifests already tracked by ISSUE-023; no project layout changes were made.
- Independent Sol/high code review: clean.

These checks establish interpreter and schema behavior only. Native runtime and
physical x86 acceptance remain deferred under ISSUE-019.

See the [plan](../plans/STAGE_2.md#s2-31dz-admit-historical-sf-only-with-pre-existing-ie-on-bounded-m32m64-stores),
[CPU contract](../specs/CPU_CONTRACT.md#s2-31dz--historical-sf-with-pre-existing-ie-for-bounded-m32m64-stores),
and [S2-31dx predecessor report](S2_31DX.md).
