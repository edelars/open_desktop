# S2-37ds — MULPS XMM0,m128 [RAX]

## Implemented boundary

The isolated `feat/s2-37ds` branch adds strict schema-v2 profile
`s2-long-mulps-memory-rax-v1` for exactly `0F 59 00`. The instruction is three
bytes, uses one retirement, and stops at RIP entry+3. It requires SSE bit 25
(not SSE2); complete fetch precedes #UD checks for missing SSE, CR0.EM or
CR4.OSFXSR, followed by #NM for CR0.TS. The profile requires XMM/MXCSR state.

MXCSR admission allows only supported bits (`0xffbf`), requires all six
exception masks, and clears DAZ, FTZ and reserved bits. All four rounding modes
and prior sticky histories are accepted. Unmasked exceptions and #XM delivery
are closed. RAX must be 16-byte aligned regardless of AC. The interpreter
checks checked canonical48 `[RAX,RAX+15]` before exactly one `read16`; alignment,
span and read failures preserve CPU/SSE/RIP and retirement.

Each lane multiplies old XMM0 by the matching memory lane using
`Binary32::multiply_raw_result(MXCSR.RC)`. IE/DE/OE/UE/PE aggregate over lanes
and OR into the old MXCSR sticky bits, preserving ZE and unrelated state. NaN
classification is performed before the helper so SNaN raises IE and NaN lanes
suppress same-lane DE. `0×∞` raises IE. Success replaces XMM0 and advances RIP
by three. JIT, NativeX86, C ABI, generic, fragment and physical x86 support are
not claimed.

The handwritten fixture has separate old XMM0 and m128 inputs. The strict
schema validator checks code bytes, budget/stop, profile controls, provenance,
expected fault/output state, memory preservation, and retirement. Its
independent integer-only `independent_mulps` oracle consumes the memory RHS.
Runner arithmetic coverage crosses seven memory-specific vectors with all
four RC settings and all 64 prior sticky histories (1,792 executions),
including NaN payload/priority, same-lane NaN plus subnormal, `0×∞`, signed
zeros/infinities, tiny/WPR/tie cases, overflow and mixed IE/DE/OE/UE/PE. It
also rejects output/control tampering and JIT/native execution or relabeling.

## Verification

- Focused fixture-v2 runner test: passed (1/1; 1,792 matrix comparisons).
- Focused decoder exact-form test: passed (1/1).
- Focused ISA execution test: passed (1/1), including exactly one read16,
  alignment #GP before data access, read-error rollback, full three-byte fetch,
  and #UD priority over #NM.
- Full suites passed: od-isa 1,843; od-testkit 455; runner 119 binary tests
  and 500 fixture-v2 tests (sequential).
- Workspace all-target offline check and formatting passed. Graphify refreshed
  to 11,514 nodes / 24,786 edges / 369 communities.
- Layout check reported only the isolated-worktree `.git` marker and two
  existing absent oracle manifests linked from `STAGE_2_REPORT.md`; this slice
  adds no filesystem paths beyond documented fixture/report files.
- Independent Sol/high code and schema review passed after correcting three
  CPU-contract anchor links; no implementation findings remain.
- Physical x86, JIT, NativeX86, C ABI, generic, fragment and unmasked/#XM
  execution were not exercised or claimed.

## Sources and related contracts

Intel SDM Rev. 093 Vol. 2A Table 2-19; Vol. 2B MULPS pp. 4-173–4-174; Vol. 1
§§4.9.1.4–.6 and 11.5.3; Vol. 3A Event 19 and Table 7-8; Intel Floating-Point
Reference Sheet v2.13. See the [plan](../plans/STAGE_2.md#s2-37ds--masked-only-legacy-long64-mulps-xmm0m128-rax),
[CPU contract](../specs/CPU_CONTRACT.md#s2-37ds--masked-only-legacy-long64-mulps-xmm0m128-rax),
[shared Stage 2 report](STAGE_2_REPORT.md#s2-37ds--mulps-xmm0m128-rax), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37ds--mulps-xmm0m128-rax).
