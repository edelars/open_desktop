# S2-37dm — masked-only Long64 MULSS XMM0,m32 [RAX]

Implemented the interpreter-only schema-v2 profile `s2-long-mulss-memory-rax-v1` for exact legacy `F3 0F 59 00`, four-byte fetch, budget one and stop RIP entry+4. The decoder rejects neighboring prefixes, REX, ModR/M/addressing forms, VEX/EVEX and altered lengths. It requires Long64, SSE (`CPUID.01H:EDX[25]`), CR0.EM=0, CR4.OSFXSR=1, CR0.TS=0 and XMM/MXCSR state. The complete instruction is fetched before #UD/#NM gates.

MXCSR requires supported mask `0xffbf`, reserved bits clear, all six exception masks set and DAZ=FTZ=0. All rounding modes and prior sticky histories are accepted. AC=1 returns local Unsupported before span validation/read; AC=0 allows unaligned m32. Checked `RAX+3` and both canonical48 endpoints precede exactly one read4. A read fault preserves CPU/XMM/MXCSR/RIP and retirement.

Execution multiplies old XMM0.low32 by the actual memory RHS through existing `Binary32::multiply_raw_result`, keeps XMM0.high96/XMM1 and unrelated state, and merges generated IE/DE/OE/UE/PE stickies. WPR tininess-after-rounding and original-NaN behavior reuse the existing MULSS policy. No arithmetic helper or broad public API was added. JIT/native/C ABI/generic/fragment and unmasked/#XM paths remain closed.

The independent raw-u32 `independent_mulss` oracle receives the memory RHS. The matrix covers 25 raw operand pairs × 4 RC modes × 64 prior sticky histories (6,400 executions); runner assertions check Interpreter backend, StopAddress, available snapshot, stop RIP and retirement=1 before result validation. Coverage includes normal/zero/subnormal/infinity/QNaN/SNaN/sign cases, both-NaN (including quiet-lhs/signaling-memory-RHS) and NaN/subnormal pairs, zero×infinity, halfway/overflow and WPR boundaries. Direct/schema tests cover MXCSR denial, gates/fetch order, AC precedence, unaligned access, canonical/span and checked-address failures, one-read rollback, provenance/result tampering, backend denial and register/neighbor regressions. The direct runtime denial cases reject missing exception masks, DAZ, FTZ and reserved MXCSR bits without retirement or data reads; a truncated faulting fetch wins over simultaneous #UD/#NM conditions with no retirement. Both-NaN and NaN/subnormal matrix pairs check original-NaN preference, IE and DE behavior.

Intel source: [SDM downloads, Rev. 093](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html); Vol. 2B MULSS pp. 4-150–4-151 (`F3 0F 59 /r`, SSE, scalar destination preservation); Vol. 2A §2.5.3/Table 2-20 p. 2-28 (Legacy SSE exception gates and unaligned scalar memory access); Vol. 1 §§11.5.2–11.5.3 (MXCSR exception flags/masks) and §11.6.6 (`MXCSR_MASK`, supported mask `0xffbf`). All six masks being set excludes unmasked delivery/#XM from this profile. No physical x86 acceptance is claimed.

Implementation and validation status are recorded in the [Stage 2 plan](../plans/STAGE_2.md#s2-37dm--masked-only-legacy-long64-mulss-xmm0m32-rax), [CPU contract](../specs/CPU_CONTRACT.md#s2-37dm--masked-only-long64-mulss-xmm0m32-rax), [shared report](STAGE_2_REPORT.md#s2-37dm--mulss-xmm0m32-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37dm--mulss-xmm0m32-rax).

## Validation

- `cargo test -p od-isa --offline`: 1,811 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --offline`: 116 unit + 494 fixture tests passed, including the 6,400-execution independent matrix.
- `cargo test -p od-x86-runner --offline`: 8 library + 4 binary tests passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed after formatting.
- `python3 tools/check_layout.py` passes in the main checkout. In the isolated worktree it reports the `.git` pointer as an unmapped root file and two pre-existing `work/stage*-oracle` links as absent because disposable `work/` artifacts are not present there.
- `graphify update .`: 11,344 nodes, 24,413 edges, 362 communities.
- Independent Sol/high review: clean, with all requested coverage corrections closed. No physical x86 acceptance is claimed.
