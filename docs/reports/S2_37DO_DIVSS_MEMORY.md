# S2-37do — masked-only Long64 DIVSS XMM0,m32 [RAX]

Implemented strict interpreter-only schema-v2 profile `s2-long-divss-memory-rax-v1` for exact legacy `F3 0F 5E 00`, budget one and stop RIP entry+4. The decoder rejects neighboring prefixes, REX, ModR/M/addressing forms, VEX/EVEX and altered lengths. The profile requires Long64, SSE (`CPUID.01H:EDX[25]`), CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1, XMM/MXCSR, and full fetch before #UD/#NM gates.

MXCSR admission is limited to supported mask `0xffbf`, reserved bits clear, all six masks set and DAZ=FTZ=0. All four RC modes and all 64 prior sticky histories are admitted. AC=1 refuses before address span checks or read; AC=0 permits unaligned m32. Checked `RAX+3` and canonical48 endpoints precede exactly one read4. Memory faults preserve architectural state and retirement.

Execution divides old XMM0.low32 by the actual memory RHS using `Binary32::divide_raw_result`, preserves XMM0.high96 and unrelated state, and merges IE/DE/ZE/OE/UE/PE. The independent raw-u32 `independent_divss` oracle reads the memory RHS. Original NaNs take priority and are quieted; ZE is raised only for finite nonzero divided by zero; DE follows invalid/ZE classification; output flags follow. WPR tininess is after rounding; exact tiny results do not raise UE. The SDM Type 3 note on Rev. 093 Vol. 2B p. 3-276 refers to VEX forms, not legacy DIVSS.

Runner coverage crosses 20 operand pairs × four RC × 64 prior stickies (5,120 executions), asserting interpreter backend, StopAddress, snapshot and retirement=1 before independent oracle comparison. Cases include both NaN orders, 0/0, infinity/infinity, zero division classification, subnormal DE, exact tiny, WPR underflow, overflow and inexact rounding. Direct checks cover gates/fetch precedence, runtime MXCSR denial, AC/span/read rollback, result/input digest tampering and JIT denial.

No physical x86 acceptance is claimed. See the [Stage 2 plan](../plans/STAGE_2.md#s2-37do--masked-only-legacy-long64-divss-xmm0m32-rax), [CPU contract](../specs/CPU_CONTRACT.md#s2-37do--masked-only-long64-divss-xmm0m32-rax), [shared report](STAGE_2_REPORT.md#s2-37do--divss-xmm0m32-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37do--divss-xmm0m32-rax).

## Validation

- `cargo test -p od-isa --offline`: 1,824 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline`: 118 unit + 495 fixture tests passed, including the 5,120-run DIVSS memory oracle matrix.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `graphify update .`: 11,405 nodes, 24,567 edges, 362 communities.
- `python3 tools/check_layout.py` reports the isolated-worktree `.git` pointer and two absent historical `work/stage*-oracle` manifest links. These are isolated-worktree environment findings outside the slice.
- Independent Sol/high review: pending.
