# S2-38al — unmasked ADDPD m128 memory profile

Implemented the immutable schema-v2 interpreter-only profile
`s2-long-addpd-memory-unmasked-rax-v1` for exact Long64 `66 0F 58 00`
(`ADDPD XMM0,m128 [RAX]`). The profile has a separate
`addpd_memory_rax_unmasked_controls` echo, SSE2 bit 26 gates, one executable
mapping, one read-only 16-byte source mapping, budget one and stop RIP=entry+4.
The existing masked-memory S2-37du and register S2-37az profiles remain
separate. Complete fetch and exact-form checks precede feature/EM/OSFXSR #UD
and TS #NM gates; noncanonical fetch spans fault before those gates.

The executor requires supported MXCSR bits `0xffbf`, reserved bits clear,
DAZ=FTZ=0, OM=UM=1, and permits every RC, IM/DM/ZM/PM setting and prior sticky
history. The aligned memory path checks both canonical48 endpoints and performs
one read16. Misalignment returns #GP(0) independently of AC. Both lanes use
pre-execution snapshots and the existing raw ADDPD semantics. IE/DE are
aggregated before masks; an unmasked pre-event commits only applicable IE/DE
and suppresses later status, both destination lanes, RIP and retirement.
Otherwise post-events merge atomically, and unmasked PE suppresses the full
destination and retirement. OSXMMEXCPT selects #XM vector 19 or SIMD-generated
#UD vector 6, with no error code. JIT, native, C ABI, generic, fragment routes
and physical x86 acceptance remain closed.

The independent integer-only raw-u64 oracle uses the fixture's actual memory
RHS. The actual interpreter matrix covers six lane representatives in both
lanes, all 16 IM/DM/ZM/PM combinations and both OSXMMEXCPT mappings at fixed
RC/sticky state: 1,152 executions. Fourteen cross-lane/boundary vectors span
all RC values, 64 prior sticky histories, all masks and both mappings: 114,688
additional executions. They include IE+DE in separate lanes, pre-event
suppression of later PE/OE, masked DE followed by unmasked PE, lane permutation,
signed QNaN/SNaN cases, exact tiny, cancellation, halfway, exponent gap and an
overflow midpoint. Same-format addition is not required to produce an
inexact-tiny case.

Four literal fixtures cover masked success, unmasked IE/#XM, DE/SIMD-#UD and
PE/#XM. Direct executor checks cover wrong-form-before-gate behavior,
#UD-before-#NM priority, noncanonical fetch/data spans, CPL0–3 execution,
AC-independent alignment, exactly one read16, pre-event aggregation and
suppression, post-event PE, prior sticky flags, and read-failure rollback.
Fixture checks cover strict profile/control validation, read-only mapping,
MXCSR constraints, control/status/result/exception tampering, mandatory
alignment at AC=0/1, and JIT/native denial.

## Verification

- `cargo fmt --all -- --check` — passed.
- Focused `od-isa` unmasked ADDPD executor test — passed.
- Focused `od-runner` interpreter event/history matrix — passed: 1,152 +
  114,688 executions.
- Focused `od-runner` literal fixture/schema/tampering/backend test — passed.
- `cargo test -p od-isa -p od-testkit -p od-runner --offline --quiet` —
  passed: od-isa 1,965; runner 143 unit and 564 fixture tests; od-testkit 461.
- `cargo check --workspace --all-targets --offline` — passed.
- `graphify update .` — passed: 12,597 nodes, 27,238 edges, 379 communities.
- `git diff --check` — passed.
- `python3 tools/check_layout.py` — blocked by existing isolated-worktree
  conditions: `.git` is an unmapped root entry, and two existing
  `STAGE_2_REPORT.md` links target absent stage-1/stage-2 work manifests. No
  layout change was made to mask those conditions.
- `cargo fmt --all -- --check` — passed before the final testkit fixture-field
  initializer update; rerun pending.
- Independent Sol/high review — pending.

No physical x86 execution is claimed.
