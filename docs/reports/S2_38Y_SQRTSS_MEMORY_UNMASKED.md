# S2-38y — SQRTSS m32 memory with unmasked exceptions

Implemented the strict schema-v2 interpreter-only profile
`s2-long-sqrtss-memory-rax-unmasked-v1` for exact `F3 0F 51 00`. It has an
independent `sqrtss_memory_rax_unmasked_controls` echo and preserves the
existing S2-37dg masked profile and S2-37aj behavior. The profile requires SSE
bit 25 only, checks complete fetch and exact form before SSE/EM/OSFXSR `#UD`
and TS `#NM`, accepts MXCSR supported mask `0xffbf` with reserved bits zero
and DAZ disabled, refuses AC=1, checks canonical `[RAX, RAX+3]`, and performs
one four-byte read.

Execution reuses the established scalar SQRTSS classification and exception
policy. Terminal invalid events suppress later events; positive subnormal DE
precedes computation and suppresses PE when unmasked; unmasked postcompute PE
commits applicable MXCSR flags while suppressing destination, RIP advance, and
retirement. OSXMMEXCPT selects `#XM` versus arithmetic `#UD`. Successful
execution preserves XMM0 high96 and unrelated architectural state. Native,
JIT, C ABI, generic, and fragment paths remain closed for this profile.

## Coverage

The runner performs all 65,536 combinations: 16 raw sources × eight IM/DM/PM
mask patterns × two OSXMMEXCPT values × four rounding controls × 64 previous
sticky histories. Sources include signed zeros and infinities, exact finite
values, a rounding neighbor, max finite, normal/subnormal boundaries, the
exact-square positive subnormal `0x00000002` (`sqrt=2^-74`), both signed
QNaNs, and positive SNaN. The matrix compares against an independent integer
raw-u32 oracle and checks full state, flags, retirement, exception vector,
destination/RIP suppression, and memory.

Seven literal fixtures cover a masked inexact result and unmasked IE, DE, and
PE under both `#XM` and arithmetic `#UD`. Additional tests cover negative SNaN
quieting, CPL 0–3, full-fetch and gate priority, AC/address denial, schema and
control-echo tampering, reserved MXCSR bits, FTZ support, and backend denial.
Focused cases also cover negative-subnormal terminal IE across IM/DM masks and
both OSXMMEXCPT mappings, negative-SNaN unmasked IE under both mappings,
`#GP(0)` comparator validation and AC-before-span precedence for masked and
unmasked SQRTSS, and rejection of writable source mappings. A MINSD regression
confirms its existing FTZ=0 restriction remains enforced.

## Validation

- `cargo test -p od-isa -p od-testkit -p od-runner --offline --quiet`: passed;
  1,938 ISA tests, 455 testkit tests, 131 runner unit tests, and 549 fixture
  tests. The runner fixture count includes the focused negative-SNaN, negative
  subnormal, signed-span, and AC-priority regressions.
- Updated matrix test: passed all 65,536 cases in 26.62 seconds.
- Focused S2-38y fixture tests: 3 passed, including negative SNaN quieting.
- Follow-up targeted MINSD, SQRTSS gate/schema, signed-span comparator,
  negative-subnormal, and negative-SNaN tests: passed individually.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `graphify update .`: passed; refreshed graph has 12,236 nodes, 26,406 edges,
  and 366 communities.
- `python3 tools/check_layout.py`: reports the linked-worktree `.git` pointer
  as an unmapped root file and two missing historical stage-oracle manifests
  referenced by `STAGE_2_REPORT.md`. These are worktree/layout environment
  findings; no layout files or unrelated reports were changed.
- Independent review: pending at author completion.

No physical x86 acceptance is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38y--exact-sqrtss-m32-memory-profile-with-unmasked-exceptions),
[stage plan](../plans/STAGE_2.md), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38y-sqrtss-memory-unmasked).
