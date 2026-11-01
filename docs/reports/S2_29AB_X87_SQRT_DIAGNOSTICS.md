# S2-29ab — finite binary80 square-root precision diagnostics

## Scope

Added `sqrt_finite_to_binary80_precision_with_diagnostics(value, mode,
precision)` in `od-isa::x87`, returning
`Option<SqrtFiniteToBinary80PrecisionResult>`. The public result contains the
raw binary80 `value`, `inexact`, and `incremented`. It accepts exactly the
existing S2-29aa domain: positive, nonzero normalized finite binary80 inputs
and precision 24, 53, or 64. Both existing bit-only APIs remain projections of
the result's `value`; their bytes and `None` domains are unchanged.

For exact decomposition `(E,q,R)`, p=64 is mathematically inexact iff
`R != 0`. At p<64, with `h=64-p` and `L=q & ((1<<h)-1)`, it is inexact iff
`L != 0 || R != 0`. Therefore p24 exact midpoint roots with `R=0` still report
inexact when discarded root bits are nonzero. `incremented` reports a one-unit
increase to the truncated positive target-precision significand before carry
normalization. These are helper diagnostics, not architectural PE or C1.

## Independent literal coverage

Literal binary80 bytes and boolean expectations cover exact roots at p=24/53/64
under all four modes, the existing p24 midpoint inputs
`0x8000030000048000` and `0x80000500000c8000`, p53 boundaries with nonzero
exact root remainder, and input `0x8000030000047fff` where direct p24 nearest
does not increment but p64 nearest does. Both latter results are inexact.
Additional inherited S2-29aa literals cover exponent/carry boundaries,
minimum-subnormal and negative-exponent roots, pseudo-denormal equivalence,
invalid input/precision rejection, projection equivalence, and immutability.

For the midpoint inputs, mode order N/Z/U/D yields `incremented` arrays
`[true,false,true,false]` and `[false,false,true,false]`, respectively. Both
inputs report `inexact=true` in all four modes despite exact `R=0`.

## Validation

- `cargo test -p od-isa finite_sqrt --offline`: 8 passed.
- `cargo test -p od-isa --offline`: 1,746 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo check -p od-isa --all-targets --offline`: passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `python3 tools/check_layout.py`: reports the isolated worktree's `.git`
  pointer and absent ignored Stage 1/2 oracle manifests tracked by ISSUE-023;
  all S2-29ab links resolve after this report was added.
- `graphify update .`: passed; refreshed to 11,018 nodes, 23,582 edges, and
  336 communities. The AST-only update includes code changes; docs paths were
  checked by `tools/check_layout.py`.
- Independent Sol/high review: pending at report authoring.

The project graph was queried from the main checkout before source inspection;
the isolated worktree initially had no graph index. Its graph was refreshed
after the source change.

## Boundaries and review

This is a mathematical helper only. No FCW/FSW, architectural exception,
FSQRT decode/execution, native/JIT/C ABI, physical x86, or performance claim was
added. No common floating-point diagnostics framework was introduced. ISSUE-019
remains open for broader x87 behavior and physical acceptance.

Independent code review is assigned separately and has not yet been completed
for S2-29ab. The preceding S2-29aa implementation review remains accurately
recorded as clean: its reviewer found no P0–P2 findings on implementation
commit `9706ed9f008c7fea78a18cd29248b03c576ed219`.

See the [S2-29ab plan](../plans/STAGE_2.md#s2-29ab-finite-binary80-square-root-precision-diagnostics-bounded-helper), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-29ab-finite-binary80-square-root-precision-diagnostics), and [shared Stage 2 report](STAGE_2_REPORT.md#s2-29ab--finite-binary80-square-root-precision-diagnostics).
