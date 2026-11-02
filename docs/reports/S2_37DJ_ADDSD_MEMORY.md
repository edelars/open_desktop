# S2-37dj — masked-only `ADDSD` XMM0,m64 [RAX]

## Implemented profile

Added the strict schema-v2 interpreter profile `s2-long-addsd-memory-rax-v1` for exact legacy `F2 0F 58 00`, instruction budget one, and stop RIP entry+4. It requires `Long64`, `SSE2` (`CPUID.01H:EDX[26]`), `CR0.EM=0`, `CR4.OSFXSR=1`, `CR0.TS=0`, and XMM/`MXCSR` state. Complete four-byte fetch precedes local `#UD` gates and the later `#NM` gate.

The profile accepts `MXCSR` bits within `0xffbf`, all six exception masks set, `DAZ=FTZ=0`, and reserved bits clear. All four RC settings and all 64 prior sticky combinations are accepted. `AC=1` returns local Unsupported before span checks or data callbacks; `AC=0` permits unaligned m64. The interpreter checks checked-add overflow and both canonical48 endpoints for `[RAX,RAX+7]` before one `read8`. A failed read preserves full CPU/SSE/`MXCSR`/RIP state and retirement.

Execution adds old `XMM0.low64` to the loaded memory RHS with `Binary64::add_raw_result(MXCSR.RC)`. It preserves `XMM0.high64`, `XMM1` and unrelated state, and ORs generated IE/DE/OE/UE/PE status into old `MXCSR` stickies. Unmasked exceptions and `#XM` are not modeled. JIT, native, `C ABI`, generic and fragment execution remain closed.

## Fixture and independent oracle

The literal fixture `s2_37dj_addsd_m64_halfway.case.json` encodes `1 + 2^-53`. It poisons `XMM1`, checks `XMM0.high64` canaries and verifies that memory remains unchanged. The independent raw integer `independent_addsd` oracle consumes the literal memory RHS; it does not use host floating point or production helpers.

Direct ISA coverage crosses four RC modes with all 64 sticky histories across halfway and adjacent values, tiny/subnormal sums, overflow, NaNs, infinities and signed zeros. It also checks gate/fetch precedence, AC refusal, an unaligned successful access, canonical endpoints, address overflow, one `read8` and read-failure rollback.

The runner regression executes seven representative operand pairs across the full RC/sticky matrix (1,792 CLI runs). Before oracle validation it asserts `StopAddress`, an available snapshot and retirement count one. It then validates each result through the independent oracle. Strict fixture schema/provenance checks, result/backend tampering, JIT/native denial and the existing register `ADDSD` fixture are covered. A generic-runner regression verifies Unsupported without state changes or data reads. The new profile is absent from default generic configurations; fragment and C ABI routes remain structurally closed at their separate API/decoder boundaries and have no dedicated S2-37dj negative tests.

## Validation

| Check | Result |
|---|---:|
| Focused decoder and direct ISA checks | 4 passed |
| od-isa full suite | 1,795 passed |
| od-testkit full suite | 455 passed |
| od-runner unit tests | 113 passed |
| od-runner fixture tests | 491 passed |
| od-x86-runner macOS host library and binary tests | 8 + 4 passed |
| Workspace all-target offline check | passed |
| cargo fmt --all -- --check | passed |
| git diff --check | passed |
| Graphify AST refresh | 11,207 nodes; 24,220 edges; 335 communities |

The layout checker reports the worktree `.git` marker and two absent historical oracle manifests already tracked by ISSUE-023. The Linux cross-target od-x86-runner check remains blocked by ISSUE-029: its existing Linux-only snapshots/fixture initializers are incomplete and serde_json is unavailable to that target. The new native profile is classified as local-only and has a denial test in the Linux-only native suite, which could not be built or run on this macOS host.

No physical x86 acceptance is claimed. Local tests establish agreement between the interpreter and the independent fixture oracle, not hardware behavior.

## Review and references

Independent Sol/high review: no correctness findings. Reviewer repeated the three then-existing focused ISA cases and all 1,792 runner matrix executions. The review identified the generic/fragment/C ABI evidence gap; the generic negative test was added, and fragment/C ABI wording now records structural closure without claiming dedicated tests. Linux native denial remains unrun under ISSUE-029.

See the [Stage 2 plan](../plans/STAGE_2.md#s2-37dj--masked-only-legacy-long64-addsd-xmm0m64-rax), [CPU contract](../specs/CPU_CONTRACT.md#s2-37dj--masked-only-legacy-long64-addsd-xmm0m64-rax), [shared Stage 2 report](STAGE_2_REPORT.md#s2-37dj--addsd-xmm0m64-rax), [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37dj--addsd-xmm0m64-rax), and [ISSUE-032](../KNOWN_ISSUES.md#issue-032--s2-37dj-lacks-physical-x86-acceptance-evidence).

Sources: Intel SDM Rev. 093 Vol. 2A `ADDSD` pp. 3-22–3-23 and legacy `SSE2` rows in Table 2-27; Vol. 1 §§4.9.1.4–.6 and 11.5.2–11.5.3; Vol. 3A Event 19/Table 7-8.
