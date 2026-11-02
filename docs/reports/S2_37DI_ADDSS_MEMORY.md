# S2-37di — masked-only ADDSS XMM0,m32 [RAX]

## Implemented boundary

The exact schema-v2 profile `s2-long-addss-memory-rax-v1` accepts only Long64 `F3 0F 58 00`, with four-byte fetch, budget one and stop RIP entry+4. It requires SSE bit 25 and applies full-fetch #UD gates before #NM for TS. XMM/MXCSR state must exist. Supported MXCSR bits are limited to `0xffbf`; all exception masks are set, DAZ and FTZ are clear, and RC plus all six old sticky bits may vary.

AC=1 is local Unsupported before address/span/read handling; AC=0 admits unaligned m32. The checked span `[RAX,RAX+3]` must not overflow and both endpoints must be canonical48. Success calls exactly one read4. Access failure leaves full state and retirement unchanged. The arithmetic is `old XMM0.low32 + memory m32`, using existing `Binary32::add_raw_result`; XMM0.high96 and unrelated state are preserved. Existing ADDSS raw status rules generate IE/DE/OE/UE/PE, which are ORed with previous sticky state. No MINSS/SQRTSS priority is copied. All JIT/native/C ABI/generic/fragment paths and unmasked/#XM behavior remain denied.

## Evidence

The literal `s2_37di_addss_m32_halfway.case.json` has `1.0` in XMM0.low32, `2^-24` in m32 memory, poisoned XMM1, and XMM0.high96 canaries. Under round-to-nearest-even, it produces `1.0` plus sticky PE and preserves high96. The independent `independent_addss` testkit oracle uses raw integer binary32 arithmetic and receives memory RHS bits; it does not use host floating point or production helpers.

Coverage includes all four RC modes × 64 prior sticky histories over midpoint/adjacent, large exponent gap, tiny/subnormal, NaN, infinity, signed zero and overflow vectors; exact encoding/malformed decode; full fetch and SSE/EM/OSFXSR/TS ordering; absent SSE state; AC-before-address; unaligned admission; canonical endpoint crossing and endpoint overflow; exactly one read4; atomic callback failure; schema/profile/control/result tampering; XMM1 source poisoning; high96 preservation; register ADDSS regression; and JIT/native denial.

An automated runner unit regression now executes the actual S2-37di schema profile in-process over 12 raw memory-RHS/input pairs × 4 RC modes × 64 sticky histories (3,072 cases). Every emitted `RunResult` goes through `validate_for`, which checks the architectural result against independent `independent_addss` using the validated guest data mapping as the RHS. This covers the memory execution profile end to end without spawning one CLI process per matrix element.

Verification on this macOS host:

- `cargo fmt --all -- --check`, `git diff --check`: passed.
- `cargo test -p od-isa --offline`: 1,787 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline`: 113 unit and 490 fixture tests passed.
- `cargo test -p od-x86-runner --offline`: 8 library and 4 binary unit tests passed; Linux-only native fixture denial is cfg-gated and did not run here.
- `cargo check --workspace --all-targets --offline`: passed.
- `python3 tools/check_layout.py`: passed on the clean main checkout. In this linked worktree it reports the worktree `.git` file as an unmapped root file and cannot resolve two pre-existing Stage 1/2 oracle-manifest links under ignored `work/` data; those links resolve in the main checkout.
- `graphify update .`: completed.
- Focused decoder, direct ISA matrix, independent oracle matrix and literal runner fixture tests: passed.
- Independent Sol/high review: clean after the profile matrix also asserted successful retirement and an available snapshot for each case; no findings remain.

Physical x86 acceptance is not claimed.

Sources: Intel SDM Rev. 093 Vol. 2A ADDSS pp. 3-24–3-25 and legacy SSE rows in Table 2-27; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8. The p. 3-25 Type 3/Table 2-20 note applies to VEX forms; legacy SSE capability and exception claims use the legacy rows.

See the [Stage 2 plan](../plans/STAGE_2.md#s2-37di--masked-only-long64-addss-xmm0m32-rax), [CPU contract](../specs/CPU_CONTRACT.md#s2-37di--masked-only-long64-addss-xmm0m32-rax), [shared report](STAGE_2_REPORT.md#s2-37di--addss-xmm0m32-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37di--addss-xmm0m32-rax).
