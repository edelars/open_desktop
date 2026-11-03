# S2-38ag — unmasked MULSS m32 memory profile

## Result

Implemented an immutable schema-v2 interpreter-only profile
`s2-long-mulss-memory-unmasked-rax-v1` for exact Long64 `F3 0F 59 00`
(`MULSS XMM0,m32 [RAX]`). It requires SSE bit 25, completes the four-byte
instruction fetch before gate faults, admits unaligned RAX when AC=0, checks
canonical48 `[RAX, RAX+3]`, and makes exactly one m32 read. AC=1 refuses before
span validation or data access. Read/span failures preserve state and
retirement.

The new profile has its own `mulss_memory_rax_unmasked_controls` schema and
SSE/CR0/CR4 controls. It requires DAZ=FTZ=0, reserved MXCSR bits clear, and
OM=UM=1 while permitting all RC modes, all prior sticky flags, and every IM,
DM, ZM, and PM mask combination. Generated IE/DE take priority and commit only
pre-event status; unmasked PE commits applicable generated status while
suppressing XMM0, RIP, and retirement. OSXMMEXCPT selects #XM versus arithmetic
#UD. Execution uses the existing raw MULSS production helper; expected results
come from the independent integer-only raw-u32 testkit oracle.

The existing masked-memory MULSS, SUBSS, and DIVSS permission predicates are
preserved. Their writable-data mapping behavior is explicitly regression
checked; only the new unmasked MULSS profile requires a read-only data mapping.

## Coverage

- 114,688 actual interpreter executions, each checked against the independent
  raw-u32 integer oracle: all 13 operand/event vectors plus an exact-tiny vector
  × 4 rounding modes × 64 incoming sticky histories × 16 IM/DM/ZM/PM
  combinations × both OSXMMEXCPT settings. Runtime vectors cover signed QNaN
  and SNaN in both source positions, QNaN+subnormal IE/DE priority,
  zero×infinity, signed zero/infinity, exact and inexact products, tiny inexact
  underflow, overflow, WPR rounding to minimum-normal, and exact tiny output
  (`min-normal × 0.5`).
- A separate 106,496-case schema/result-validator matrix uses 13 independently
  computed oracle vectors across the same controls and histories. It covers
  signed QNaN/SNaN positions, zero×infinity, signed zero/infinity, exact and
  inexact products, tiny inexact results, overflow, and WPR rounding to
  minimum-normal. This validates schema admission and result tamper checks; it
  does not count as interpreter execution coverage.
- Three handwritten literal fixtures exercise unmasked IE, unmasked PE, and
  WPR rounding to minimum-normal. Runner checks include control/state/status/
  exception/retirement tampering and JIT/native denial.
- Direct ISA probes cover gate and full-fetch ordering, TS, CPL0–3 exception
  delivery, AC-before-address, unaligned read4, one callback, canonical and
  wrapping-span rejection, read-failure rollback, and IE/DE pre-event versus PE
  post-event suppression.
- After the matrix expansion, package suites passed: `od-isa` 1,959;
  `od-testkit` 461; `od-runner` 138 unit and 561 fixture tests. The full
  114,688-execution runtime matrix completed in 9.46 seconds. Workspace
  all-target offline check, formatting, Graphify refresh (12,460 nodes, 27,060
  edges, 366 communities), and diff check passed.
  `tools/check_layout.py` reported the isolated worktree `.git` marker
  and two pre-existing links from the aggregate Stage 2 report to absent
  `work/` oracle manifests.

## Boundaries

This is interpreter-only local fixture evidence. It does not establish physical
x86 acceptance. Native, JIT, C ABI, generic, and fragment routes remain denied.
Unmasked OE/UE delivery remains outside the profile because OM=UM=1 are
required.

## Validation and review

- Status: package suites, workspace check, formatting, Graphify refresh, and
  diff check passed. The layout checker limitation is described above.
- Independent review: pending parent review before integration.
