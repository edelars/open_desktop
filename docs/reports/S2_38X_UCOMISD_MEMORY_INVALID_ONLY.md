# S2-38x — UCOMISD m64 memory invalid-only profile

## Scope and implementation

Added immutable schema-v2 interpreter-only profile
`s2-long-ucomisd-memory-rax-invalid-v1` for exact Long64 legacy
`66 0F 2E 00` (`UCOMISD XMM0,m64 [RAX]`). It requires one executable code
mapping, one read-only eight-byte data mapping at RAX, budget one, stop RIP at
entry+4, and its own `ucomisd_memory_rax_invalid_controls` echo. S2-37cp's
masked UCOMISD profile and S2-38w's COMISD invalid-only profile remain
separate and unchanged.

The executor reuses the exact binary64 comparison-memory engine with
`quiet_nan_raises_ie=false` and `reject_subnormals=true`. QNaN is unordered
without a new IE; SNaN raises IE in either operand. Masked IE commits sticky IE
and CF/ZF/PF unordered flags. Unmasked IE commits only sticky IE and raises
`#XM` or arithmetic `#UD` according to OSXMMEXCPT, suppressing RFLAGS, RIP and
retirement. Any subnormal operand, including a NaN/subnormal mixture, returns
atomic Unsupported after the successful eight-byte read. SSE2, EM, OSFXSR,
TS, AC, canonical-span and read ordering use the shared checked execution path.

Six literal fixtures cover finite greater, QNaN with IM clear, masked SNaN,
unmasked SNaN `#XM`, unmasked SNaN arithmetic `#UD`, and NaN/subnormal refusal.
The independent raw-u64 result oracle checks both NaN operand positions and
classes × IM × OSXMMEXCPT × all 64 prior sticky histories (1,024 combinations),
plus all RC values, QNaN+SNaN both orders, prior IE alone, finite/infinity/
signed-zero and binary64 boundaries, gate priority, schema/control/result
tampering, and backend denial. Direct ISA checks cover CPL0–3, one eight-byte
callback, AC refusal vs unaligned AC-clear reads, both NaN classes/positions,
unmasked exception suppression, and atomic subnormal refusal.

## Verification

- `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo test -p od-isa --offline --quiet`: 1,934 passed.
- `cargo test -p od-testkit --offline --quiet`: 455 passed.
- `cargo test -p od-runner --offline --quiet`: 130 unit and 542 fixture tests passed.
- Focused runner test includes the full 1,024-case matrix and all six fixtures.
- `git diff --check`: passed.
- `python3 tools/check_layout.py`: reports the linked worktree `.git` pointer
  as an unmapped root file and two pre-existing broken links to ignored Stage
  1/2 oracle manifests; those files are absent from this isolated worktree.
- `graphify update .`: passed (12,183 nodes, 26,301 edges, 366 communities).
- Independent Sol/high review is clean. It found that the finite-greater fixture
  contained QNaN; commit `8580b70` corrects it to 2.0 > 1.0, after which review
  completed with no remaining findings.
- No physical x86 execution or acceptance is claimed. ISSUE-033 remains open.

## Main integration

Integrated as `fe54bbd` plus fixture correction `8580b70` with S2-31ee.
Independent Sol/high review is clean. Combined main verification passed: od-isa
1,937, od-testkit 455, od-runner 130 unit + 543 fixture, workspace offline
check, fmt, layout, diff check, and Graphify 12,150/26,369/366. See the
[main Stage 2 integration report](STAGE_2_REPORT.md#main-integration-s2-31ee-s2-38x).

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38x--exact-ucomisd-memory-invalid-only-profile),
[Stage 2 plan](../plans/STAGE_2.md#s2-38x--exact-ucomisd-memory-invalid-only-profile),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38x-ucomisd-memory-invalid-only-profile).
