# S2-41b — bounded LIDT oracle protocol

**Status:** proposed documentation-only protocol; no executable harness,
fixture, serialized profile, native admission, or physical result is added.
All four physical cases and machine integration are `not_run`. This prepares
S2-41 steps 1–2; parent S2-41 and Stage 2 remain open.

## Scope and sources

The only instruction under test is bare Long64 `0F 01 5C 24 06`,
`LIDT m16&64 [RSP+6]`, length five. Its expected IDTR, RIP and completion
behavior is taken from the existing
[S2-27d CPU contract](../specs/CPU_CONTRACT.md#s2-27d--exact-long64-lidt-rsp6).
That contract pins Intel SDM Rev.093, September 2026, Vol. 2A `LGDT/LIDT`
(Order No. 253666-093US) and Vol. 3A §§2.4.1, 3.5.2, 4.5.1, 4.5.3 (Order
No. 253668-093US), retaining official source links. Those existing references
were cross-checked against the local contract and S2-27d report; this slice
does not claim a fresh PDF verification or add new SDM section citations.
Before implementation or acceptance, separately review the proposed SIDT,
INT3, #GP, trap-frame and stack machinery below against the pinned SDM.
That source review is an open prerequisite, not a closed harness contract.

The [Ubuntu mapping](../reports/STAGE_2_LINUX_PROFILE.md#s2-27d--exact-ubuntu-lidt-candidate)
locates these exact bytes at `ffffffff8133ed4e` in
`__pi_startup_64_load_idt` of the selected Ubuntu 7.0.0-38.38 ELF, with a
pseudo-descriptor limit `0x01ff` and base `ffffffff83a04000`. The isolated
source-to-binary match and skipped `MOV SS` do not establish a contiguous
executed startup path. They prove neither LIDT runtime execution nor IDT/event
behavior or Linux boot. Synthetic addresses below are independent test inputs,
not reconstructed kernel runtime state.

The [S2-41a LGDT protocol](S2_41A_LGDT_ORACLE.md) supplies the evidence-tier
pattern. LIDT requires its own observation design because it replaces the IDT
that the next completion marker or fault will use.

## Evidence tiers and ownership

| Tier | What it can establish here | Owner / present boundary |
|---|---|---|
| Linux user-mode oracle | No admitted LIDT case; root privileges do not make a process CPL0, and a host signal is not a raw architectural vector/error code | S2-40 user-mode harness; native admission stays closed |
| CPU contract / fixture-memory backend | Local semantics, callback ordering, state commit and profile-policy rejection | S2-27d / `od-isa`; existing checks in the [report](../reports/STAGE_2_REPORT.md#s2-27d--exact-long64-lidt-rsp6), not rerun here |
| Independent privileged oracle | Actual IDTR read by SIDT before/after, entry CPL and raw completion/fault context | Future S2-41 harness; four proposed cases, physical `not_run` |
| Integrated machine | Paging, GDT/IDT/TSS/stack mappings and delivered events interacting with the CPU implementation | Stage 4 backend with S2-41 validation; `not_run` |

S2-27d's one explicit ten-byte `GuestMemory` read at CPL0 and absence of a data
callback before CPL1–3 rejection are project API observations, not a count of
hardware bus reads or speculative accesses. `idtr == None` returning
`Unsupported` is local fail-closed policy, not an architectural LIDT
precondition. Injected memory errors check local atomicity; they are not
observed architectural page faults or page-table walks. S2-27d has no serialized
fixture profile. These new literal inputs have not been executed by either the
local CPU tests or a physical harness in this documentation task.

## Fixed inputs and observation prerequisites

Each case starts from a fresh harness snapshot. Only CPL changes; no result of
one case supplies the next case's IDTR. Values are hexadecimal except explicit
decimal lengths and counts.

| Input | Literal value / requirement |
|---|---|
| Mode and entry | Long64, one CPU; valid CS/SS and descriptor caches for the selected CPL; entry RIP `0x100000` |
| Test bytes | At `0x100000`: `0F 01 5C 24 06`; instruction boundary `0x100005` |
| RSP / source | RSP `0x401000`; SS-default source `[RSP+6..RSP+15]` is `0x401006..0x40100f` |
| Operand bytes | `FF 0F 00 10 30 00 00 00 00 00`: little-endian limit `0x0fff`, base `0x301000` |
| Old IDTR | Limit `0x0fff`, base `0x300000`; record independent pre-entry SIDT bytes |
| Flags / other state | RFLAGS `0x2`, no debug stepping or injected test event; fixed recorded GPR, segment and control-state sentinels |
| Completion marker | Harness-owned `CC` (INT3) at `0x100005`, outside the five-byte test region |

Both limits deliberately cover all 256 sixteen-byte IDT slots. The base differs
so the successful load is observable, while all vectors retain space for
valid gates. These cases do not test changing the IDT limit. The ten-byte
source is canonical, readable and contained in one page. All input bases are
canonical; noncanonical cases are outside this protocol.

The future manifest must pin code and handler bytes, page tables/permissions,
old/new IDT images at `0x300000` / `0x301000`, GDT and TSS images, selectors,
ring-transition/IST stacks, test stack `[0x400000, 0x402000)`, and separate
handler code/output/stack regions. Code, source and test-stack permissions must
permit the intended CPL entries; handler storage must not overlap the operand.
The operand lies above the entry RSP, so an instrumentation push, call, stack
adjustment or wrapper before LIDT would change the effective address. Final
entry must establish exactly the recorded RSP and land directly on the test
bytes. Capture setup state before that entry; do not add an in-region probe.

**Both old and new IDTs must have compatible verified #BP, #GP and fallback
gates**, backed by valid unchanged GDT selectors, TSS state and handler stacks.
The #BP gate must permit the marker at every tested CPL. The manifest and
review must account for exception stack frames, stack alignment, privilege
changes and any IST choice; neither trap entry nor register-save/output writes
may overwrite `0x401006..0x40100f`. No selected mapping may make a secondary
fault look like the test result. With RFLAGS.IF clear, external interrupt
handling still cannot be assumed irrelevant: NMI and unexpected faults need
a reviewed fallback path. An unexpected event invalidates the case.

LIDT itself loads the pseudo-descriptor; it does not read or validate gates in
the loaded IDT. Subsequent observation does use those gates. A successful LIDT
followed by an invalid new #BP gate is therefore a harness failure, not evidence
of an LIDT fault. Exact tables, stacks, fallbacks and trap-entry code remain to
be implemented and independently reviewed; missing prerequisites block a run.

## Four-case matrix

Every row uses the full shared input above. Owner: future S2-41 privileged
harness. Protocol status: `proposed`; physical result for each row: `not_run`.

| Case | CPL | Expected architectural result | IDTR / instruction boundary | Completed test instructions |
|---|---|---|---|---|
| `lidt-cpl0-valid` | 0 | Success, no LIDT fault | Limit `0x0fff`, base `0x301000`; post-LIDT RIP `0x100005` | 1 retired instruction |
| `lidt-cpl1-gp` | 1 | `#GP(0)`, vector 13, saved CS identifies CPL1 | Old limit `0x0fff`, base `0x300000`; saved fault RIP `0x100000` | 0 |
| `lidt-cpl2-gp` | 2 | `#GP(0)`, vector 13, saved CS identifies CPL2 | Same old IDTR; saved fault RIP `0x100000` | 0 |
| `lidt-cpl3-gp` | 3 | `#GP(0)`, vector 13, saved CS identifies CPL3 | Same old IDTR; saved fault RIP `0x100000` | 0 |

LIDT preserves RSP `0x401000`, the operand and unrelated CPU state. The fault
rows do not commit IDTR, RIP advancement or retirement. Preservation refers to
the tested instruction boundary, not live handler registers or the changes
made by exception entry. Retain raw trap state separately and document how
entry GPR/RSP/flags are recovered without treating handler writes as LIDT output.

## Proposed independent observation

A future privileged harness owns the tables, mappings and CPL transitions.
A Linux root process or a ptrace/host-signal report is insufficient.

1. Validate the marker control path **under both old and new IDT**, at every
   tested CPL, before accepting any LIDT case. Enter the marker directly with
   its intended entry context; establish raw #BP delivery and the observation
   stack without executing the instruction under test. Verify #GP and fallback
   gates and stack paths independently as harness prerequisites. These controls
   are separate results, excluded from the four LIDT cases.
2. Restore a fresh case snapshot and verify entry CS/CPL, RSP, code, all ten
   source bytes and table/mapping images. Record old IDTR with SIDT before case
   entry. The observer must use CPU observations, without calling the project's
   decoder/evaluator or copying expected operand bytes into observed IDTR.
3. On success the marker uses the **new** IDT. Record raw vector 3, no error
   code and saved RIP `0x100006` (entry+6). Report the completed LIDT boundary
   separately as `0x100005` (entry+5). The INT3 is observation machinery and
   does not count as a tested instruction.
4. For CPL1–3, #GP uses the **old** IDT. Record raw vector 13, present error
   code zero, saved RIP `0x100000` and matching saved CS/CPL; no marker may have
   been reached. Preserve the trap frame and GPRs before instrumentation
   clobbers them. A different vector, location, error code or secondary trap
   is a failure/unavailable observation, never a substitute for `#GP(0)`.
5. Inside either handler, capture actual IDTR with SIDT into separate output
   storage **before restoring IDTR or other test context**. Preserve raw ten
   bytes and decode limit/base separately. A post-restore SIDT cannot establish
   what LIDT did. Save operand bytes and recoverable instruction-boundary state
   without allowing trap storage to alter the descriptor.
6. Infer completed test instructions from the verified straight-line region
   and raw trap location: one for the marker path, zero for the fault path.
   This is an architectural completion assertion, not a PMU or total harness
   count. Keep raw saved RIP so INT3's entry+6 cannot be reported as LIDT's
   entry+5. Restore/cleanup belongs to the harness and is recorded separately.

The design above has not been implemented or experimentally verified. Source
review of SIDT/INT3/#GP and frame semantics, table/gate/stack validation and
successful control runs are mandatory before physical acceptance. No raw
observations means `not_run`, never a pass inferred from local CPU checks.

Future evidence must retain source revision, harness/build and CPU identity,
exact input/table/mapping/handler images or digests, pre/post raw SIDT bytes,
entry CS/CPL/RSP and context, raw trap vector/error-code presence/value/frame,
completion boundary/count, controls, per-case outcome and repeat identity.
Run at least two fresh-state repeats per case; preserve mismatches and missing
observations. Expected literals and actual results must remain separate. No
new evidence schema, fixture or runner is introduced here.

## Open boundaries and acceptance

Noncanonical operand spans or loaded bases, unreadable pages, mixed-fault
priority, other displacement/prefix/addressing forms, other modes, broader
paging/event delivery, transitions and system integration remain open under
S2-27/S2-41 and Stage 4. This protocol does not extend admission, change the CPU
contract or prove Linux startup/boot. Hardware and integration remain `not_run`.

Documentation acceptance covers four explicit cases, evidence separation,
source links and the observation prerequisites. Physical acceptance additionally
requires reviewed source semantics and a working independent harness with raw
stable case/control observations. Even then only this instruction boundary is
covered; parent S2-41 and Stage 4 integration do not close automatically.
The [S2-41 plan](../plans/STAGE_2.md#s2-41-независимо-проверить-privilegedmode-сценарии)
retains its owner-approval requirement for future bare-metal work on the sole
Mac mini. This proposal contains no execution procedure or authorization for
disk/bootloader changes, reboot, guest boot, SSH or hardware execution.
