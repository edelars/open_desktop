# S2-37dg — SQRTSS m32 memory profile

Date: 2026-11-01

## Implemented scope

The schema-v2 interpreter implements only the Long64 instruction
`SQRTSS XMM0,m32 [RAX]`, exact bytes `F3 0F 51 00`, in profile
`s2-long-sqrtss-memory-rax-v1`. It fetches all four bytes before the SSE/EM/
OSFXSR #UD checks and the TS #NM check. The bounded profile requires SSE,
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1, and XMM/MXCSR state. Other encodings,
addressing forms, prefixes, VEX/EVEX and non-XMM0 forms remain refused.

The admitted local MXCSR subset has all six exception masks set, DAZ and FTZ
clear, reserved bits clear (`mxcsr & !0xffbf == 0`), and accepts every RC value
and prior sticky history. Unmasked exception delivery and #XM behavior are
outside this profile. With AC set the profile returns Unsupported before
validating the address or invoking a data callback. With AC clear it permits
unaligned m32 access. It validates canonical48 for `[RAX,RAX+3]` before one
four-byte read; address arithmetic overflow or a noncanonical span raises
#GP(0), and a memory-read failure leaves architectural state and retirement
unchanged.

The binary32 input comes only from memory and is evaluated by
`Binary32::sqrt_raw_result(MXCSR.RC)`. Success changes XMM0.low32 only, ORs
generated IE/DE/PE into the existing sticky bits, advances RIP by four, and
preserves XMM0.high96 and unrelated state. Negative nonzero inputs raise IE;
positive subnormals raise DE, with PE reported for inexact rounded results.
JIT, native, C ABI, generic and fragment execution remain closed.

The schema-v2 validator uses its independent raw-u32 integer oracle, separate
from the production square-root helper. Direct ISA coverage exercises all four
rounding modes over representative exact/inexact, signed-zero, infinity, NaN,
negative and subnormal inputs; it also checks seeded sticky flags, high96
preservation, unaligned AC=0 reads, exact read width/count, AC refusal,
instruction gates, canonical/overflow span refusal and read-failure rollback.
The literal fixture supplies binary32 2.0 from an unaligned address and checks
the rounded low result, preserved high96, PE, unchanged memory and backend
denial. Provenance tampering tests verify that JIT/native cannot claim success,
gate exceptions or AC refusal snapshots; those backends validate only with
unsupported results that omit snapshots. No physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2B SQRTSS pp. 4-669–4-670; Vol. 1 §4.9.2
and §§11.5.2.1–.2, 11.5.3.1–.2; Vol. 3A Table 7-8; Intel Floating-Point
Reference Sheet v2.13, pp. 1–2. See the [plan](../plans/STAGE_2.md#s2-37dg--exact-legacy-long64-sqrtss-xmm0m32-rax),
[CPU contract](../specs/CPU_CONTRACT.md#s2-37dg--masked-only-long64-sqrtss-xmm0m32-rax),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37dg--sqrtss-xmm0m32-rax).

## Verification

- `cargo test -p od-isa sqrtss_memory_rax --offline`: 2 passed.
- `cargo test -p od-isa --offline --quiet`: 1,773 passed.
- `cargo test -p od-testkit --offline --quiet`: 454 passed.
- Focused `od-runner` SQRTSS literal fixture: 1 passed.
- `cargo test -p od-runner --offline --quiet`: 112 unit tests and 488 fixture tests passed.
- `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `git diff --check`: passed.
- `graphify update .`: passed; refreshed 11,149 nodes, 23,917 edges, 344 communities.
- `python3 tools/check_layout.py`: reports the isolated worktree's `.git` marker as an unmapped root file and two existing links from `STAGE_2_REPORT.md` to unavailable `work/` manifests. No layout files changed.

Independent Sol/high code review is pending. The profile remains a local
interpreter/schema-v2 slice; no native, JIT or physical x86 execution is claimed.
