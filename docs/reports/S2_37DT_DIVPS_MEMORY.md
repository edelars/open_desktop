# S2-37dt — DIVPS XMM0,m128 [RAX]

## Implemented scope

The exact legacy `0F 5E 00` form has a Long64 interpreter profile
`s2-long-divps-memory-rax-v1`. It uses SSE feature bit 25, fetches the complete
instruction before #UD/#NM checks, and applies SSE/EM/OSFXSR before TS. The
profile requires supported MXCSR bits `0xffbf`, six masks set, reserved bits,
DAZ and FTZ clear; every RC and prior sticky pattern is admitted.

Memory access requires 16-byte alignment regardless of AC, checked `RAX+15`
and canonical48 endpoints, then exactly one read16. The execution snapshots
old XMM0 and divides its four binary32 lanes by corresponding memory lanes
through `Binary32::divide_raw_result`. It aggregates IE/DE/ZE/OE/UE/PE,
preserves unrelated state, and commits no state on alignment/span/read faults.
Other forms, unmasked/#XM handling, and JIT/native/C ABI/generic/fragment paths
remain unsupported.

## Oracle and coverage

The result validator calls the independent raw-u32 rational `independent_divps`
oracle with actual memory RHS lanes. The literal fixture includes 1/3, 0/0,
0/infinity and 1/1 lanes. The runner test crosses all four RC modes with all 64
prior sticky histories and adds qNaN/ZE suppression, sNaN/DE suppression,
finite/zero, infinity/zero, subnormal, 1/3 and overflow vectors. It checks
schema/result tampering, malformed bytes, SSE gate, misalignment, noncanonical
span, JIT/NativeX86 relabeling and full snapshot comparison. A direct ISA test
checks exact memory access width/count, four-lane result, MXCSR flags, XMM1
preservation and read-error rollback. Decoder coverage rejects adjacent
prefixes, REX, other ModR/M, truncated input, VEX and EVEX forms.

## Validation

- Focused decoder test: **passed**, 1/1.
- Focused direct ISA memory execution test: **passed**, 1/1.
- Focused runner/profile test: **passed**, 1/1, including 256 RC/sticky cases
  and special values, midpoint/min-normal boundaries, overflow and mixed flags.
- Full `cargo test -p od-isa --offline`: **passed**, 1,847 tests.
- Full `cargo test -p od-testkit --offline`: **passed**, 455 tests.
- Runner unit tests: **passed**, 119 tests.
- Full serialized `cargo test -p od-runner --offline --test fixture_v2 -- --test-threads=1`:
  **passed**, 502 tests in 336.19 seconds.
- `cargo fmt --all -- --check` and `cargo check --workspace --all-targets --offline`:
  **passed**.
- `git diff --check`: **passed**. Graphify refresh passed with 11,541 nodes,
  24,841 edges and 357 communities.
- `python3 tools/check_layout.py`: **blocked by pre-existing worktree issues**:
  the isolated worktree `.git` pointer is reported as an unmapped root file,
  and two historical Stage 1/2 oracle manifest symlinks target absent files
  already tracked by ISSUE-023.
- Independent Sol/high review is pending.

No physical x86 acceptance is claimed. The profile does not cover complete
unmasked overflow/underflow exception delivery.

See [CPU contract](../specs/CPU_CONTRACT.md#s2-37dt--masked-only-long64-divps-xmm0m128-rax), [Stage 2 plan](../plans/STAGE_2.md#s2-37dt--masked-only-legacy-long64-divps-xmm0m128-rax), [shared report](STAGE_2_REPORT.md#s2-37dt--divps-xmm0m128-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37dt--masked-only-long64-divps-xmm0m128-rax).
