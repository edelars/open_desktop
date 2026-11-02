# S2-37dn — legacy MULSD XMM0,m64 [RAX]

## Boundary

Adds strict schema-v2 interpreter-only profile `s2-long-mulsd-memory-rax-v1` for exactly `F2 0F 59 00`. The instruction fetch is four bytes, budget is one and the stop address is entry+4. The profile requires Long64/SSE2, CR0.EM=0, CR4.OSFXSR=1, CR0.TS=0 and XMM/MXCSR state. Full fetch precedes #UD/#NM checks.

MXCSR accepts only supported bits `0xffbf`, all six exception masks set, DAZ/FTZ clear; all RC modes and sticky histories are admitted. AC=1 refuses locally before span/read; AC=0 allows unaligned m64. Checked `RAX+7` and canonical48 endpoints precede exactly one read8. The execution multiplies original XMM0.low64 and the actual memory RHS through `Binary64::multiply_raw_result`, preserving the existing original-NaN and WPR tininess-after-rounding behavior. It merges IE/DE/OE/UE/PE and preserves XMM0.high64, XMM1 and unrelated state. Runtime memory faults leave state and retirement unchanged. JIT/native/C ABI/generic/fragment and unmasked/#XM paths remain closed.

## Tests

- Exact decoder form and neighboring encoding rejection.
- Direct execution checks for successful unaligned one-read behavior, read failure rollback, MXCSR runtime denials without data access, full-fetch-before-gate ordering, SSE2/#NM gates, AC-before-span refusal, and canonical/overflow span faults.
- Runner literal verifies memory operand selection, XMM canaries, unchanged RAM, StopAddress and retirement.
- Runner raw-u64 oracle matrix covers 25 operand pairs × four RC × 64 sticky histories (6,400 cases), including both QNaN/SNaN orders, zero×infinity, subnormals, overflow and WPR boundary neighbors. StopAddress, snapshot and retirement are asserted before oracle validation.
- Schema rejects nonexact encoding/length, wrong memory width, unsupported MXCSR, absent/invalid controls and missing provenance; JIT/native are denied.

## Validation status

Validation passed: focused decoder/direct ISA checks 2/2, full `od-isa` 1,817, `od-testkit` 455, runner 116 unit + 495 fixture tests, workspace all-target offline check, formatting and diff check. Final Graphify AST refresh: 11,375 nodes, 24,493 edges and 363 communities. Isolated layout validation reports the worktree `.git` marker and two unavailable historical oracle manifests recorded under ISSUE-023. Independent Sol/high review is clean: od-isa MULSD 5/5 and runner matrix 1/1 (6,400 cases). No physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2B MULSD pp. 4-148–4-149; Vol. 2A §2.5.3/Table 2-20; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and 11.6.6. See [CPU contract](../specs/CPU_CONTRACT.md#s2-37dn--masked-only-long64-mulsd-xmm0m64-rax), [Stage 2 plan](../plans/STAGE_2.md#s2-37dn--masked-only-legacy-long64-mulsd-xmm0m64-rax), [shared report](STAGE_2_REPORT.md#s2-37dn--mulsd-xmm0m64-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37dn--mulsd-xmm0m64-rax).
