# S2-37dx — DIVPD XMM0,m128 [RAX]

Implemented the exact legacy `66 0F 5E 00` Long64 memory form behind a separate `DivpdMemoryRax` decoder operation, `DivpdControls` selector, and `run_with_divpd_memory_rax_controls` API. The existing register-pair DIVPD profile remains selected separately. The local schema-v2 profile is `s2-long-divpd-memory-rax-v1`.

Execution fetches all four bytes before SSE2/CR0.EM/CR4.OSFXSR `#UD` and CR0.TS `#NM` gates. Runtime admission requires MXCSR supported bits `0xffbf`, all six exception masks set, reserved bits clear, and DAZ/FTZ clear; all RC modes and previous sticky histories are allowed. It checks 16-byte alignment irrespective of AC, checked `RAX+15`, and canonical48 endpoints before one atomic `read16`. The memory callback must succeed before XMM0/MXCSR change. Both old XMM0 binary64 lanes divide by the matching memory lane using `Binary64::divide_raw_result`; IE/DE/ZE/OE/UE/PE aggregate across lanes. JIT/native/C ABI/generic/fragment and unmasked/#XM remain closed.

The handwritten fixture uses memory as the actual RHS, distinguishes both output lanes, and poisons XMM1. The strict result validator uses an independent raw-u64 rational oracle. Its matrix crosses 14 operand pairs × four RC modes × 64 sticky histories (3,584 executions), with gate, alignment, canonical span, tampering, and backend denial cases. An od-isa direct test additionally checks complete-fetch precedence over gates at CPL0–3, #UD/#NM priority, misalignment/noncanonical faults, atomic read failure, and the largest aligned canonical base whose inclusive endpoint is `u64::MAX`. A dedicated overflow fixture is not representable in the v2 memory schema because its data region would itself overflow; checked addition is present in the executor, and 16-byte alignment makes an aligned base-plus-15 overflow unreachable in u64.

## Validation

- `cargo test -p od-isa --offline`: 1,866 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline -- --test-threads=1`: 120 unit + 509 fixture tests passed (serialized aggregate, 381.06 seconds). An earlier parallel aggregate had five fixture subprocess spawn `ENOENT` failures; each failed case passed individually and the complete serialized rerun passed.
- `cargo test -p od-x86-runner --offline`: 8 library + 4 binary tests passed.
- Focused direct ISA fetch/gate/address/read rollback test: 1 passed.
- Focused DIVPD decoder test: 1 passed.
- Focused memory DIVPD oracle/backend test: 1 passed (3,584 matrix executions).
- Existing DIVPD register-pair regression: 1 passed.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check`: passed.
- `python3 tools/check_layout.py`: reports the isolated worktree `.git` pointer and two absent historical oracle manifests already tracked by ISSUE-023. The new report links resolve.
- Graphify AST refresh: 11,655 nodes / 25,110 edges / 370 communities.
- Independent Sol/high review: clean after adding the interpreter-only result-backend guard and JIT/native relabel regression checks.

No physical x86 acceptance is claimed. Normative sources: Intel SDM Rev. 093 Vol. 2A DIVPD pp. 3-267–3-269 and legacy SSE rows in Table 2-19; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8. The Type 2 note on p. 3-269 concerns VEX forms and is not applied to legacy DIVPD.

See [CPU contract](../specs/CPU_CONTRACT.md#s2-37dx--masked-only-legacy-long64-divpd-xmm0m128-rax), [Stage 2 plan](../plans/STAGE_2.md#s2-37dx--masked-only-legacy-long64-divpd-xmm0m128-rax), [shared report](STAGE_2_REPORT.md#s2-37dx--divpd-xmm0m128-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37dx-divpd-xmm0m128-rax).
