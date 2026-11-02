# S2-37dp — masked-only Long64 DIVSD XMM0,m64 [RAX]

Implemented exact legacy `F2 0F 5E 00` with a strict schema-v2 profile, SSE2 gate,
all-masked MXCSR and raw binary64 division using the actual memory RHS. AC=1
refuses before address-span/read validation; AC=0 checks overflow and canonical48
endpoints before one read8. The result updates only XMM0.low64 and merges IE/DE/ZE/
OE/UE/PE sticky flags. JIT/native, generic decode profiles and unmasked delivery
remain closed.

The runner matrix covers 24 raw operand pairs × 4 RC settings × 64 prior sticky
histories (6,144 executions). Runtime checks cover SSE2 gating, AC refusal and
canonical span faults. Schema checks cover exact opcode/length, MXCSR, controls,
provenance and output tampering; backend relabeling to JIT and NativeX86 is
rejected. Full validation passed: `od-isa` 1,827, `od-testkit` 455, runner 118 unit + 496
fixture, x86-runner 8 library + 4 binary, workspace all-target offline check,
formatting, diff check and Graphify AST refresh (11,431/24,629/352). The isolated
layout script reports its `.git` pointer and two absent historical oracle
manifests already tracked as ISSUE-023; main checkout layout passes. Independent Sol/high review is clean with no remaining findings.

Normative source: Intel SDM Rev. 093 Vol. 2A DIVSD pp. 3-273–3-274. The Type 3
note is VEX-specific, not part of legacy DIVSD. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-37dp--masked-only-long64-divsd-xmm0m64-rax), [Stage 2 plan](../plans/STAGE_2.md#s2-37dp--masked-only-legacy-long64-divsd-xmm0m64-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37dp--divsd-xmm0m64-rax).
