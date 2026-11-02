# S2-37du — ADDPD XMM0,m128 [RAX]

This slice adds a strict schema-v2 interpreter profile for exact legacy
Long64 `66 0F 58 00`. It is separate from the existing ADDPD register-pair
profile and from ADDPS: it has its own decoded operation, execution-profile
slot, API entry point, fixture profile and runner dispatch.

The instruction fetches all four bytes before SSE2/CR0.EM/CR4.OSFXSR #UD and
CR0.TS #NM gates. The profile requires XMM/MXCSR state and masked-only MXCSR:
reserved bits clear, supported mask `0xffbf`, DAZ/FTZ clear, all six exception
masks set. RAX is required to be 16-byte aligned regardless of AC. The checked
canonical48 span `[RAX,RAX+15]` is validated before exactly one `read16`.
Gate, alignment, span and read failures preserve architectural state and
retirement. Unmasked/#XM, JIT, native, C ABI, generic and fragment execution
remain closed.

Success adds old XMM0 to two Binary64 lanes from the actual memory operand
using MXCSR.RC, aggregates IE/DE/OE/UE/PE into existing sticky state, preserves
ZE and unrelated state, writes XMM0 and advances RIP by four. The independent
testkit validator reads RHS lanes from the fixture's memory bytes and uses the
independent raw-u64 `independent_addpd` oracle; it validates full state,
memory, controls, exit and retirement.

The handwritten fixture has `1.0+2.0` and `2.0+5.0` lanes. ISA tests cover
every RC mode over 64 sticky combinations, halfway rounding, lane flag
aggregation, SNaN/subnormal behavior, fetch/gate ordering, invalid MXCSR with
no data read, alignment/canonical-span failures and read rollback. Runner
coverage checks the literal result, output tampering, #UD/#GP atomicity and
JIT/native denial.

## Verification

Passed focused decoder/ISA tests (3); full `od-isa` (1,852), `od-testkit`
(455), and `od-runner` (119 unit + 504 fixture) suites; workspace
`cargo check --workspace --all-targets --offline`; `cargo fmt --all -- --check`;
and `git diff --check`. Graphify refreshed to 11,569 nodes / 24,911 edges /
372 communities. The project layout check exits 1 in this linked worktree for
the `.git` marker and two already-missing oracle manifests referenced by the
shared Stage 2 report; this slice adds no paths. Independent Sol/high review
and commit are pending. Physical x86 acceptance is not claimed.

See the [Stage 2 plan](../plans/STAGE_2.md#s2-37du--masked-only-legacy-long64-addpd-xmm0m128-rax),
[CPU contract](../specs/CPU_CONTRACT.md#s2-37du--masked-only-long64-addpd-xmm0m128-rax),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37du-addpd-m128-memory-profile).
