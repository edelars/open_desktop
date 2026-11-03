# S2-41a — bounded LGDT oracle protocol

**Status:** documentation-only protocol; no new executable harness, fixture,
native admission, or physical result. All four physical cases are `not_run`.
S2-41 and Stage 2 remain open. This protocol defines the next small independent
check, not acceptance of the privileged/system profile.

## Scope and sources

The sole tested instruction is bare Long64 `0F 01 55 F6`,
`LGDT m16&64 [RBP-10]`, length four. Expectations below come from the existing
[S2-27a CPU contract](../specs/CPU_CONTRACT.md#s2-27a--exact-long64-lgdt-rbp-10):
Intel SDM Rev.093, September 2026, Vol. 2A `LGDT/LIDT` (Order No.
253666-093US), and Vol. 3A §§2.4.1, 3.5.2, 4.5.1, 4.5.3 (Order No.
253668-093US), with official source links retained there. This protocol does
not broaden that contract or decide fault priority outside the four cases.

The [Ubuntu mapping](../reports/STAGE_2_LINUX_PROFILE.md#s2-27a--exact-ubuntu-lgdt-candidate)
locates these bytes at `ffffffff83f132f9` in the selected
`vmlinux-7.0.0-38-generic.debug`, in the path through
`__pi_startup_64_setup_gdt_idt` and `native_load_gdt`. That is static evidence,
not an observed Linux path. The synthetic addresses below deliberately do not
claim to reproduce that kernel's runtime state.

## Evidence tiers and owners

| Tier | What it can establish here | Owner and present status |
|---|---|---|
| Linux user-mode oracle | No admitted LGDT case. A root process remains user-mode; a host signal does not expose architectural fault vector/error code or prove CPL0 execution | S2-40 user-mode harness; LGDT admission stays closed |
| CPU contract / fixture-memory backend | Project semantics, atomic state commit, callback ordering, and local policy rejection | S2-27a / `od-isa`; existing checks recorded in the [report](../reports/STAGE_2_REPORT.md#s2-27a--exact-long64-lgdt-rbp-10), not rerun in this docs slice |
| Independent privileged oracle | Actual GDTR before/after via SGDT, entry CPL and test context, completion or raw trap-frame fault | S2-41 privileged harness; four cases specified below, harness pending, physical `not_run` |
| Integrated machine | Guest paging, GDT/IDT/stack mappings, events and transitions interacting with the CPU implementation | Stage 4 machine backend with S2-41 validation; pending, `not_run` |

The existing CPU checks report one explicit ten-byte memory callback at CPL0
and no data callback before CPL1–3 rejection. Callback counts are observations
of the project API, not hardware bus reads or microarchitectural accesses.
`state.gdtr == None` and non-admitted prefixes returning `Unsupported` are
local profile-policy checks, not architectural fault expectations. Injected
read faults test local no-commit behavior, not real trap delivery. The existing
S2-27a report does not claim that the new literal cases below were executed.

## Fixed input shared by all four cases

Each case starts from a reset harness snapshot and executes only the four-byte
LGDT as the instruction under test. No previous case supplies its GDTR.
Numbers below are hexadecimal unless a decimal length is explicit.

| Input | Literal value / requirement |
|---|---|
| Execution mode | Long64; one CPU; CPL is the case parameter; valid CS/SS and descriptor caches for that CPL |
| Test code | At `0x100000`: `0F 01 55 F6`; entry RIP `0x100000`; completion boundary `0x100004` |
| RBP | `0x20000a`, so signed disp8 `F6` selects SS-default source `0x200000` |
| Operand image | Ten readable bytes at `0x200000`: `7F 00 00 10 30 00 00 00 00 00`; limit `0x007f`, base `0x301000` |
| Initial GDTR | Present local state: limit `0x00ff`, base `0x300000`; independent harness confirms the same pair with SGDT before entry |
| RSP and flags | RSP `0x401ff0` in a mapped test stack; RFLAGS `0x2` at entry; no pending debug stepping or externally injected test event |
| Other state | Fixed recorded GPR/segment/control-state sentinels; no intended writes to source bytes, test GPRs, flags, segment caches, or unrelated CPU state |

The future harness manifest must pin actual GDT/IDT entries, selectors,
TSS/ring-transition stacks, page tables, mapping permissions, code bytes and
handler bytes before execution. Fixed candidate mappings are code at
`0x100000`, operand at `0x200000`, old/new GDT at `0x300000`/`0x301000`,
IDT at `0x302000`, test stack `[0x400000, 0x402000)`, and separate handler
code/stack/output pages. The relevant code, operand and test-stack pages must
be accessible for every tested CPL. All ten operand bytes and both bases are
canonical and readable; none crosses a page boundary. The old/new GDTs must
contain identical valid descriptors for all selected code/data/TSS selectors
within the smaller limit, so loading the new GDTR cannot break observation.
These are harness prerequisites; LGDT itself neither reads the GDT target nor
reloads descriptor caches. Exact table images and trap-entry machinery remain
to be implemented and reviewed; absent prerequisites block a physical run.

## Case matrix

Every row uses the complete shared input above, including exact encoding
`0F 01 55 F6`, RBP, RIP, operand image and old GDTR. Only CPL changes.
Observable fields are raw pre/post SGDT bytes, entry CS/CPL and RBP, completion
or fault location, trap vector and error-code presence/value, saved CS/RIP,
and test-instruction completion count. Each row belongs to the independent
privileged tier; owner is the S2-41 harness task, status `specified`, physical
result `not_run`.

| Case | CPL | Expected architectural result | Updates / preservation | Completion count |
|---|---|---|---|---|
| `lgdt-cpl0-valid` | 0 | Success; no LGDT fault | GDTR becomes limit `0x007f`, base `0x301000`; post-LGDT RIP `0x100004`; RBP `0x20000a`, operand and unrelated state preserved | 1 retired test instruction |
| `lgdt-cpl1-gp` | 1 | `#GP(0)`, vector 13; saved fault RIP `0x100000`, saved CS identifies CPL1 | Old GDTR limit `0x00ff`, base `0x300000` preserved; RBP, operand and unrelated pre-fault state preserved; no RIP advancement | 0 |
| `lgdt-cpl2-gp` | 2 | `#GP(0)`, vector 13; saved fault RIP `0x100000`, saved CS identifies CPL2 | Same old GDTR and state preservation; no RIP advancement | 0 |
| `lgdt-cpl3-gp` | 3 | `#GP(0)`, vector 13; saved fault RIP `0x100000`, saved CS identifies CPL3 | Same old GDTR and state preservation; no RIP advancement | 0 |

State preservation describes the LGDT boundary, not the handler's live
registers, stack writes, or exception-entry flag changes. Raw trap state must
be retained separately; the observer must not compare handler-modified state
as though it were the tested instruction's output.

## Independent observation protocol

A minimal future CPL0 trap harness owns the GDT, IDT, TSS and mappings above.
It establishes the requested CPL with recorded CS/SS descriptors and snapshots
all inputs before entering the case. Neither running as Linux root nor a
ptrace/host-signal result satisfies this prerequisite.

1. Before entry, the harness uses SGDT to capture the old ten-byte GDTR image,
   verifies code/operand bytes and entry state, and records the case identity.
   The observer obtains results from the CPU, without calling Open Desktop's
   decoder/evaluator or deriving GDTR from the expected operand image.
2. The proposed completion marker is a harness-owned `INT3` byte at
   `0x100004`, immediately after the test region. Its IDT gate must allow all
   tested CPLs, with valid handler and privilege-transition stacks. Successful
   execution reaches that marker: record raw vector 3, absent error code and
   saved RIP `0x100005`, then report the reached LGDT boundary separately as
   `0x100004`. The marker is observation machinery, excluded from the test
   instruction count. A reviewed marker control case must establish this
   trap path before any LGDT result is accepted.
3. A failure handler records the raw vector, error-code presence/value,
   saved RIP and CS from the trap frame, and preserves entry GPRs before
   instrumentation clobbers them. For CPL1–3, only vector 13, present error
   code zero, original RIP and matching CPL satisfy the case. No completion
   marker may have been reached. Unexpected traps are failures, not a mapped
   equivalent of `#GP(0)`.
4. On either trap path, SGDT records the actual current GDTR **before** any
   restoration. Save raw descriptor bytes and decode limit/base separately.
   A post-restore SGDT sample is not evidence of preservation. Handler setup
   and cleanup are outside the test region and are recorded independently.
5. Count completed **test** instructions from the verified straight-line
   region and observed trap location: one on the completion path, zero on
   the fault path. This is an architectural completion assertion, not a PMU
   reading or a count of all harness instructions. Retain raw saved RIP so
   the success marker's `+5` is never mislabeled as the LGDT's `+4`.

The marker/trap machinery above is a proposed observation design, not an
implemented or verified harness. Review must validate its trap gates, stack
switching, context preservation and control case against the SDM before any
physical run. Lack of raw observations is `not_run`, never a pass inferred
from local CPU tests.

A future evidence record must carry source revision, harness/build identity,
CPU identity, exact table/mapping/input images or their digests, raw pre/post
SGDT, entry context, trap frame, completion count, per-case outcome and repeat
identity. Use at least two fresh-state repeats per case, preserving mismatches
and unavailable observations. Independent expected literals remain separate
from actual observations. No new evidence schema or runner is created here.

## Open boundaries and acceptance

Noncanonical source spans or loaded bases, unreadable operands, mixed-fault
priority, prefixes/other addressing forms, other CPU modes, segment-cache
transitions, full paging/events integration and Linux boot remain outside this
protocol. They are still owned by the broader S2-27/S2-41 and Stage 4 work;
this slice neither resolves nor newly demonstrates a defect in those paths.
The [S2-41 plan](../plans/STAGE_2.md#s2-41-независимо-проверить-privilegedmode-сценарии)
retains its separate owner-approval requirement for any future bare-metal
work on the sole Mac mini. This document contains no execution procedure and
does not authorize disk changes, guest boot, reboot or hardware execution.

Documentation acceptance checks the four rows, explicit evidence tiers,
source links and preserved open gates. Physical acceptance additionally needs
a reviewed working independent harness and raw stable observations for all
four cases. Even those observations would cover only this instruction boundary;
they do not close the integrated-machine criterion or parent S2-41.
