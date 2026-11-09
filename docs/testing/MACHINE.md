# Методика проверки машины: текущий срез

S4-00a evidence capture 2026-11-05 сверяет binary hashes по
[S2-40d](../reports/S2_40D_ARTIFACT_DIGEST_RECHECK.md), фиксирует recovered
Ubuntu 7.0.0-38.38 source/config hashes, совпадающие Build IDs и равенство 38
общих allocated file-backed ELF sections. Это не аутентифицирует mirror download и не
связывает source/config с binary; полный список ограничений и команд находится
в [S4-00a report](../reports/STAGE_4_S4-00A_2026-11-05.md). S4-00/S4-01 и
полная source provenance остаются открытыми.

## Область

Эта методика покрывает S4-02—S4-10a и bounded S4-06b/S4-06c/S4-11b: валидируемую physical
map, production RAM owner, committed-write notification, синхронную MMIO/PIO
bus, диагностический CPU memory bridge, uncached translation probe, bounded
tracked RAM access и два CPU-connected paged MOV forms. Она не проверяет полную
CPU paging matrix, guest exception delivery, APIC, JIT, firmware или Linux.

## Локальный набор

Команды выполняются из корня checkout:

```text
cargo fmt --all -- --check
cargo check -p od-machine --all-targets --offline
cargo test -p od-machine --offline -- --test-threads=1
cargo test -p od-isa --locked --offline page_fault
git diff --check
```

Тесты `od-machine` должны подтверждать:

1. checked range overflow, overlap и physical-width boundary;
2. width 64 с последним адресом `u64::MAX`;
3. общий RAM budget до allocation;
4. zeroed backing и little-endian scalar accesses;
5. hole/ROM/reserved rejection;
6. cross-region access без частичной записи и без callback;
7. ровно одно уведомление после успешной полной RAM-записи;
8. обычные guest-byte accesses, пересекающие детерминированную 16 KiB границу,
   без предположения о host page size.
9. MMIO widths 1/2/4/8, PIO widths 1/2/4 и отказ PIO Qword;
10. PIO endpoint `65536`, oversized range rejection и overlap/binding checks;
11. unknown/misaligned/read-only device accesses без device side effects;
12. отказ до dispatch для cross-window access без partial RAM/device effects.
13. execution regression через `od_isa::execute::run` и
    `IdentityMemoryAdapter`: fetch/load/store, unmapped fault, CPU state/RIP,
    retirement, RAM и committed callback;
14. explicit rejection atomic/fence operations в diagnostic flat mode.
15. four-level 4 KiB literal page tables: geometry, canonicality before reads,
    physical36/CR3, present/reserved/PS, cumulative permissions, WP/NXE,
    literal PFEC, trace, table-read errors and no A/D mutation;
16. PTE mutation and CR3 changes are observed on the next call, proving no TLB
    or translation cache in the probe.
17. `od-isa` PFEC helper matches literal nonpresent, write, reserved, NX-fetch
    and descriptor-table cases; record vector/error/address/RIP fields stay
    distinct and retain full `AccessContext`.
18. The `od-machine` orchestration converts only probe page faults; translated,
    noncanonical, unsupported-page-size and table-read results remain distinct.
    Page tables (including A/D bits) remain byte-for-byte unchanged.
19. Tracked RAM access commits missing A bits immediately during a valid walk,
    preserves those effects on later nonpresent/reserved/protection/table-read
    failures, commits D only for an allowed write after full RAM operand
    validation, and commits the operand last.
20. The tracked API accepts Read/Write widths 1/2/4/8 and Fetch remains
    byte-only; current success cases cover Read Dword, Write Byte/Word and
    Fetch Byte. Already-set A/D bits do not cause duplicate writes or callbacks,
    ROM/reserved/MMIO errors remain distinct, and cross-page/unsupported
    RAM-only accesses have no operand effect.
21. Present PS entries at PDPT/PD return the exact unsupported-page-size result
    without descendant or operand access; pure probes remain mutation-free, and
    tracked ancestor A updates are retained in order while the rejected entry
    remains unchanged.
22. Nonpresent+PS keeps nonpresent precedence; PML4 bit 7 remains a reserved
    bit, PT bit 7 remains admitted for 4 KiB, and reserved-address/NXE cases do
    not invent huge-page support or change the admission boundary.
23. Same-page Read/Write widths 1/2/4/8 cover the last fitting offsets and exact
    bytes; crossing requests return `CrossesPage` before any walk with empty
    trace/journal, zero A updates, no callback and unchanged RAM.
24. Crossing admission wins over noncanonical or inaccessible-table candidates
    and is independent of prospective RAM/RAM, RAM/hole and RAM/MMIO targets;
    it never splits a device or operand access.
25. A successful same-page call remains committed when a later crossing call is
    rejected. Adjacent caller-issued Fetch bytes use separate reports; a later
    fault does not erase the earlier byte or its A/D/callback effects.
26. `Long64ProbeConfig::from_cpu_state` requires CR0 before CR4, preserves raw
    CR0/CR4/CR3/EFER inputs through existing validation, and returns exact
    missing/invalid configuration errors without changing CPU state or memory.
27. Rebuilding snapshots after CR0/CR4 or explicit CR3 changes observes the new
    translation context while the old snapshot remains unchanged; explicit EFER
    inputs are preserved and validated; distinct roots produce distinct literal
    physical translations.
28. The experimental pure-translation cache passes preflight parity against the
    uncached probe, caches only successful 4 KiB walks, reports Hit versus Walk,
    and leaves pure-probe RAM unchanged; release performance evidence belongs
    to the separate [S4-13a report](../reports/STAGE_4_S4-13a_2026-11-05.md).
29. S4-14a verifies that `TranslationProbeCache::clear` is idempotent and does
    not write page-table RAM or A/D bits; a successful entry follows
    `Walk -> Hit -> clear -> Walk -> Hit` with literal translation metadata.
30. Failed replacement is not retained: the failed request returns `Walk`, the
    cache is empty, and the next matching request walks before becoming a hit.
31. Ending the cache scope permits a page-table remap and construction of a new
    cache, which observes the new physical page; recreated caches also observe
    leaf RW tighten/relax and NX Fetch rights with PFEC `0x07` and `0x15`.
32. Explicit page-table writes report their committed `(address, length)` range;
    cache probe itself produces no callbacks or page-table A/D mutation.
33. S4-21a runs real fetch/load/store bytes through `IdentityMemoryAdapter`,
    preserves CPU state and the `rflags_defined` mask across one budget versus
    multiple quanta, and returns `WorkLimit` without automatic continuation.
34. Pause/resume leaves CPU state unchanged; STOP is idempotent, wins before
    fetch and at an execution poll point, and prevents later `run` calls.
35. S4-21a keeps halted, architectural fault, unsupported instruction, backend
    memory error and stop-address outcomes distinct; it does not claim HLT
    wakeup, reset, IRQ, MMU or JIT lifecycle.
36. S4-22a checks zero/rounding/large virtual-clock conversions, atomic time/TSC
    overflow rejection and split-advance equivalence; time does not advance from
    execution, budgets, polling or host calls.
37. Bounded RDTSC uses the caller-driven clock and preserves EDX:EAX, RIP and
    `rflags_defined` across split budgets. TSD/CPL denial, fetch-boundary failure,
    zero budget and invalid defined masks remain explicit outcomes.
38. S4-23a verifies fixed-capacity one-shot deadlines: equal deadlines preserve
    insertion order, reschedule gets a new position, `deadline <= now` is due,
    and `pop_due` removes at most one event.
39. Duplicate/full/not-found operations, cancellation of due events, `u64`
    extremes and sequence rebasing leave the queue unchanged or ordered by the
    documented contract; VirtualClock readiness does not advance time.
40. S4-23b checks that `EventGeneration` starts at zero, every queue operation
    rejects a stale generation before lookup/capacity checks or mutation, and
    extracted events are validated before use. Successful reset clears pending
    events and sequence while retaining capacity; stale reset and generation
    exhaustion preserve entries, sequence, capacity and clock. Deadline/order
    semantics remain unchanged, and reset never advances `VirtualClock`.
41. S4-23c checks bounded UART RX payload capacity and dispatch ordering:
    generation, STOP, event budget, then readiness. Deadlines and equal-deadline
    order remain owned by `EventQueue`. Successful injection removes exactly one
    event/payload; `ReceiverBusy` preserves the earliest event, payload,
    deadline and order for retry. Reset invalidates stale pending work without
    changing `VirtualClock`, CPU state or HLT state.
42. S4-22b checks that `PagedVcpuSession` owns one explicitly configured
    `VirtualClock`, RX readiness uses only its `now_ns`, and neither run nor
    polling advances time. Split and whole advances match; pause, STOP and
    terminal states reject advancement, while active guest HLT permits clock
    progression without wakeup or RX dispatch. Reset preserves time; invalid
    frequency and checked arithmetic overflow do not mutate state.
43. S4-23d checks the owner-facing RX controls: stale generations are rejected
    before lifecycle/queue admission even after reset and key reuse; cancel and
    reschedule retain `EventQueue` ordering; ReceiverBusy preserves payload
    semantics; duplicate/full/not-found and pause/STOP/HLT/terminal failures
    leave queue contents, CPU and clock unchanged. Next-deadline queries
    are read-only in all states. No event wakes HLT.
44. S4-23e checks private capacity-one replacement, cancellation and
    generation invalidation of previously extracted events. Wrong capacity,
    stale generation and generation exhaustion leave the queue unchanged.

## S4-23d generation-aware paged RX control

`cargo test -p od-machine --lib paged_vcpu_session::tests --offline --
--test-threads=1` passed 11 tests. They check stale schedule/cancel/reschedule
requests after successful reset and key reuse, and that failed reset preserves
the old generation. Due-event cancellation and earlier/later/equal-deadline
rescheduling retain the queue ordering contract. Under `ReceiverBusy`, a
pending payload can be moved or cancelled without duplicate delivery. Stale,
duplicate, full, not-found and lifecycle errors preserve queue contents, CPU
state and clock; a read-only next-deadline query remains available. Paused,
stopped, terminal and halted owners reject current-generation mutations, and no
event wakes HLT. `cargo check -p od-machine --all-targets --offline`, workspace fmt
and diff check passed. Independent Sol/high review found no code findings. See
[S4-23d report](../reports/STAGE_4_S4-23D_2026-11-07.md).

## S4-23e single-slot deadline replacement

`cargo test -p od-machine --lib event_queue::tests --offline` passed 16 tests.
They cover replacing a full single slot, replacing after extraction, cancelling
pending/extracted events, stale generation, wrong capacity and generation
exhaustion atomicity. `cargo check -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check` passed. Independent
Sol/high review found no code findings. This private queue operation does not
yet schedule or dispatch a Local APIC timer. See the
[S4-23e report](../reports/STAGE_4_S4-23E_2026-11-07.md).

## Интерпретация

Успешные локальные тесты подтверждают только перечисленный Rust-контракт,
flat identity integration, tracked RAM-only effects и targeted `od-isa`
PFEC/record tests, pure translation probe, S4-10a rejection boundary, S4-11a
cross-page admission and bounded S4-11b qword read, S4-12a snapshot bridge, S4-13a pure-cache experiment,
S4-14a cache lifecycle boundary, S4-21a diagnostic vCPU session и S4-22a
caller-driven clock/RDTSC boundary, S4-22b paged-session time ownership,
S4-23a bounded deadline queue, S4-23b reset-generation boundary, S4-23c
bounded UART RX event delivery and S4-23d owner-facing RX control.
Они не являются physical x86 evidence и не
подтверждают, что guest 4 KiB
страницы уже имеют независимые paging permissions. Отдельные failure values не
являются автоматически архитектурными `#PF`.

После изменения кода Graphify обновляется командой `graphify update .`. При
переходе к MMU/CPU-срезам каждый тест должен дополнительно сохранять fault state,
CR2, A/D effects и source of expected result.

## S4-01a: diagnostic CPU-to-RAM baseline

The [S4-01a report](../reports/STAGE_4_S4-01A_2026-11-05.md) records a fresh
run of the exact CPU-through-RAM integration test and a separate immutable
page-walk snapshot control. Run both exact commands from that report and
require one executed test per command. The integration contract covers
`od_isa::execute::run → IdentityMemoryAdapter → MachineBus → PhysicalMemory`
in diagnostic flat identity mode (`linear == physical`); it is not a Linux
machine manifest. This baseline does not establish CPU-integrated paging,
`#PF` delivery/retry, JIT, privileged hardware behavior or Linux boot. The
captured host page size is metadata only; it does not parameterize guest test
memory.

## S4-06b: bounded CPU-connected paged MOV

Baseline revision: `b2a093c`. The reviewed code files are listed in the
[S4-06b report](../reports/STAGE_4_S4-06B_2026-11-05.md). From the repository
root, reproduce the package-level checks with:

```text
cargo test -p od-isa --locked --offline
cargo test -p od-machine --locked --offline
cargo check --workspace --all-targets --locked --offline
cargo fmt --all -- --check
python3 tools/check_layout.py
git diff --check
```

The independent `gpt-6.1-sol/high` review reran the two package test commands:
`od-isa` passed 2,014 unit tests and `od-machine` passed 85 tests. The author
reported 8 `PagedMov` CPU tests, 3 legacy CR0 tests, 2 legacy CR4 tests, and
the `paged_mov_step` integration target's 8 tests passing; the package-level
runs include these regressions. Workspace check, formatting, layout and diff
checks passed. The review approved the complete code diff with no P0–P3
findings.

Coverage includes both exact encodings, one-step retirement, CR0/CR4 admission,
`Explicit` origin with preserved CPL, `rflags_defined`, literal non-identity
translations, PFEC and saved RIP, A/D and callback ordering, aliasing,
unchanged read buffers on error, non-partial operand writes, and distinct
architectural/backend/unsupported outcomes.

This does not establish full paging, TLB/invalidation, CR2 commit or exception
delivery/retry, cross-page operands, REP/stack/atomic semantics, JIT, IRQ/devices,
SMP or Linux boot. ISSUE-054 remains open for cross-page/partial progress,
REP/RMW and broader rollback consumers.

## S4-06c: CPU-connected paged MOV to MMIO

The [S4-06c report](../reports/STAGE_4_S4-06C_2026-11-05.md) and manifest
record the focused device-dispatch and ordering evidence. The tests use the
existing `step_paged_mov` path with a literal guest mapping from the operand
GVA to a bound MMIO range. The device double records `DeviceAccess`, payload
and event order alongside committed-write callbacks.

The positive cases require a nonzero device read value to reach the MOV
destination, a write to carry the exact qword and device offset/width, exactly
one device operation, and A/D callbacks before MMIO dispatch. Negative cases
require permission failure to avoid the device call and device backend failure
to preserve CPU state and retirement count while retaining prior A/D effects.
The MMIO device contract requires failure before visible device-side effects.

Focused reproduction, check results, reviewer findings, and open boundaries
are recorded in the S4-06c report and JSON manifest. This does not test cross-page
device transactions, retries, device read/modify/write, exception delivery,
TLB/JIT, IRQs or the complete paging matrix.

## S4-11b: CPU-connected cross-page qword load

The [S4-11b report](../reports/STAGE_4_S4-11B_2026-11-05.md) and evidence
manifest record all seven offsets of an 8-byte operand across a 4 KiB boundary.
The fixture maps the two virtual pages to distinct, non-contiguous RAM pages.
Positive cases check the assembled little-endian qword, RAX/RIP, retirement,
preserved flags, no D-bit update and the ordered A-update journal.

Negative cases fault on the second page as not-present or supervisor-only,
verify the exact `PageFaultRecord`/PFEC/address/RIP/context, preserve the CPU
destination and caller buffer, and retain A updates committed by earlier walks.
RAM+hole and RAM+MMIO combinations must fail without device dispatch or a
partial operand value. A page-table alias case confirms both walks and A
updates precede operand sampling. Cross-page stores and device splitting remain
outside this method.

## S4-11c: CPU-connected cross-page dword read

`od_machine::step_paged_mov_dword_read` admits a separate exact Long64 `8B 03`
(`MOV EAX,[RBX]`) profile. Reproduce with
`cargo test -p od-machine --test paged_mov_step --locked --offline s4_11c`.
The same-page RAM case is the control; crossing offsets `0xffd`, `0xffe` and
`0xfff` use two non-contiguous RAM mappings. Tests check zero-extension, RIP+2,
single retirement, unchanged qword profile neighbors, alias sampling after
both A-bit updates, no D update, exact second-page #PF data and refusal of
cross-page MMIO dispatch. This is a synthetic installed-Long64 profile, not a
full guest or hardware acceptance.

## S4-11d: CPU-connected cross-page instruction fetch

Reproduce with `cargo test -p od-machine --test paged_mov_step --locked --offline s4_11d`.
The exact `8B 03` profile places the opcode at linear page offset `0xfff`, the
ModRM byte on a second non-contiguous RAM code page and the dword operand on a
separate RAM page. Success checks the normal single retirement. Second-fetch
nonpresent and NX faults check PFEC `0x10`/`0x11`, the faulting linear byte,
saved RIP, full CPU rollback, no retirement and no operand-page walk. A hole or
MMIO second code page must fail without device callback. A decoder unit control
places one-byte HLT at the last byte of a page and verifies no next-page fetch;
HLT is not admitted by the CPU profile. This does not establish general
cross-page instruction-fetch support.

## S4-11e: CPU-connected cross-page word read

Reproduce with `cargo test -p od-machine --test paged_mov_step --locked --offline s4_11e`
and `cargo test -p od-machine --lib --locked --offline failed_second_cross_page_walk_keeps_caller_buffer_unchanged`.
The exact `66 8B 03` (`MOV AX,[RBX]`) profile checks same-page RAM and an operand
starting at offset `0xfff` across two non-contiguous RAM pages. Success preserves
RAX[63:16], updates AX, advances RIP by 3 and retires once. CPL0 non-present and
CPL3 supervisor-only second-page faults verify exact PFEC, fault byte, saved RIP,
no CPU/buffer commit or retirement, prior A updates and no D update. RAM holes,
same-page/cross-page MMIO, alias walk-then-sample and profile exclusions are
covered. CPL0 and CPL3 require AC=0. This is a synthetic bounded profile, not
general cross-page memory or hardware acceptance.

## S4-11f: CPU-connected cross-page word instruction fetch

Reproduce with `cargo test -p od-machine --test paged_mov_step --locked --offline s4_11f`.
The exact `66 8B 03` profile executes only at CPL0 with AC clear and NXE enabled.
Starts at offsets `0xffe` and `0xfff` fetch from non-contiguous RAM code pages;
the operand remains within a separate RAM page. Offset `0xffd` is the same-page
control with the next page absent. Check exact second-page non-present/NX fetch
faults (`0x10`/`0x11`), original RIP and linear fault byte, saved CPU state, A
updates, no D, and no operand walk or retirement. A missing or MMIO code page
must not dispatch a device. A case with both a fetch fault and unavailable
operand verifies fetch priority. The existing S4-11e profile remains restricted
to same-page instruction fetch. Reject other encodings, CPL3 and AC=1 through
both the specialized and generic profile entries. Reject a word operand at
data offset `0xfff` after decode and before any operand walk; reject CR0-only
profile entry through both run and step APIs before fetch. This is not general
cross-page fetch support.

## S4-24a: bounded COM1 polling TX

Baseline revision: `6aeb2dd` (the reviewed UART changes were present in the
shared worktree after this baseline). The [S4-24a report](../reports/STAGE_4_S4-24A_2026-11-05.md)
records the bounded profile and review result. Reproduce the focused checks
from the repository root:

```text
cargo test -p od-machine --lib uart16550::tests --locked --offline
cargo test -p od-machine --test uart16550_polling --locked --offline
cargo test -p od-machine --lib bus::tests --locked --offline
cargo check -p od-machine --all-targets --locked --offline
cargo fmt --all -- --check
python3 tools/check_layout.py
git diff --check
```

Recorded results: 7 UART unit tests, 2 UART PIO integration tests and 4
existing `MachineBus` tests passed; the package all-targets check, formatting,
layout and diff checks passed. Independent `gpt-6.1-sol/high` review approved
the code diff with no P0–P3 findings.

The integration path binds the device to COM1 and checks byte PIO through the
production bus, DLAB divisor selection without accidental TX, the literal
`THRE/TEMT` transitions, raw byte completion, busy/unsupported access atomicity,
and reset through a handle that aliases the bus-owned state. This verifies only
the synchronous bounded TX endpoint. Baud timing, FIFO, IRQ4, Linux serial
compatibility, guest boot and scheduler behavior are not covered. Parent
S4-24 remains open.

## S4-24b: bounded CPU PIO and COM1 RX

The [S4-24b report](../reports/STAGE_4_S4-24B_2026-11-05.md) records the
accepted CPU-to-PIO slice and independent review. The bounded profile is
`DiagnosticCpl0PioU8V1`: only bare `EC`/`EE` in Long64 CPL0, with CR0/CR4
`None`; fetch uses flat identity RAM. Each successful call performs one byte
PIO callback and one retirement. `IN AL,DX` preserves upper RAX and consumes
the single injected UART RX byte; `OUT DX,AL` uses the existing bounded TX
holding/shift path. Failed fetch or PIO operations do not retire or mutate CPU
state.

Recorded verification from the implementation and independent review:

```text
cargo test -p od-isa --locked --offline                  # 2014 unit + 7 integration passed
cargo test -p od-machine --locked --offline              # 98 passed
cargo check -p od-isa --all-targets --locked --offline   # passed
cargo check --workspace --all-targets --locked --offline # passed
cargo fmt --all -- --check                              # passed
git diff --check                                         # passed
```

The targeted PIO integration tests cover successful byte IN/OUT, exactly one
callback and retirement, upper-RAX preservation, RBR read-clear, `LSR.DR`,
unmapped-port errors, fetch errors, unsupported instructions, CPL rejection
and unchanged CPU state on failures. UART RX accepts one injected byte,
reports data ready while it is pending, rejects a second injection until the
guest reads the first, and returns `DeviceError::WriteOnly` when the empty RBR
is read. The full `od-machine` suite and `od-isa` package tests were reported
successful; the independent `gpt-6.1-sol/high` review approved the complete
code slice with no P0–P3 findings.

These results do not establish IOPL/TSS checks, other I/O widths, immediate or
REP/string forms, IRQ delivery, APIC, Linux serial-driver compatibility or
Linux boot. Parent S4-24 remains open.

## S4-24c: bounded paged CPU PIO

The [S4-24c report](../reports/STAGE_4_S4-24C_2026-11-05.md) records the
accepted paged CPU-to-PIO slice and independent review. `step_paged_pio_u8`
uses profile `PagedCpl0PioU8V1`: bare `EC`/`EE`, Long64 CPL0, with CR3/EFER
validated by the existing `Long64ProbeConfig` four-level 4 KiB configuration.
Fetch follows guest page tables with `AccessOrigin::Explicit`. Literal fault
coverage expects PFEC `0x10` for not-present and `0x11` for NX protection.

The CPU entry accepts an already-selected `GuestMemory` and performs only
CPU-side CR0/CR4 and profile admission. The machine entry validates CR3/EFER
through the existing `Long64ProbeConfig` and supplies the real page-aware
memory adapter. The machine acceptance executes `MOV RAX,[RBX]` then `OUT DX,AL` with no host
mutation between steps and passes the first result's definedness into the
second. UART output is checked against the guest-loaded byte. On a PIO failure,
CPU/definedness are unchanged while already committed A/D writes and callbacks
remain. Full tests passed: `od-isa --all-targets` — 2,014 unit, 6 paged and 7
legacy tests; `od-machine --all-targets` — 105 tests, including 7 paged.

Recorded additional checks: package checks, formatting, diff check and layout
check passed; workspace check, JSON/link validation and independent
`gpt-6.1-sol/high` review also passed with no P0–P3 findings. These checks do not establish a mixed-instruction runner,
exception delivery/CR2 update, IOPL/TSS, IRQ/APIC, scheduler, baud timing, JIT
or Linux behavior. Parent S4-24 remains open.

## S4-24d: guest UART RX→IN→OUT echo

`cargo test -p od-machine --lib paged_vcpu_session::tests --offline --
--test-threads=1` passed 13 tests, including strict profile selection,
profile-preserving reset and receiver release after guest `IN`. The runner CLI
suite passed 9 tests: RX values `0x00`, `0x80`, `0xff`, exact RAX/RIP/TX
prefixes under budgets and cancellation, empty-RBR PIO failure, mismatched or
absent expectation, invalid bytes/options, and filenames equal to option names.
The build CLI suite passed 2 tests, including identical ELF and raw artifacts
from two builds followed by the real builder→runner echo path.

`cargo check -p od-runner --all-targets --offline`, formatting and diff checks
passed. `python3 tools/check_layout.py` remains blocked by the pre-existing
unmapped root `smoke/` (`ISSUE-056`). Independent Sol/high code and documentation
reviews passed without remaining findings. This proves one-shot polling echo;
it does not prove guest LSR polling loops, IRQ/APIC, HLT wakeup, baud timing or
the full S4-24 acceptance. See [report](../reports/STAGE_4_S4-24D_2026-11-07.md).

## S4-25a: fixed-edge Local APIC register core

Run `cargo test -p od-machine --lib local_apic::tests --offline --
--test-threads=1` for the focused register/state matrix. The tests attach the
device through `MachineBus` at `0xFEE00000` and cover reset, software
enable/disable with retained IRR/ISR, TPR/PPR priority classes, pending
inspection, one-time acknowledge, same-vector retrigger while in service, EOI,
read-only banks, unsupported offsets/width/alignment/reserved values and
rejection atomicity. `cargo check -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check` also passed.

This is local device evidence only. The xAPIC version register, LVT, base-MSR
control, default board registration, IOAPIC, level interrupts, timers, CPU IRQ
delivery, handler execution, IDT/IF/shadow behavior, HLT wakeup and Linux boot
are not covered. The selected register behavior is based on Intel SDM release
093, Volume 3A, Chapter 13. See the [slice report](../reports/STAGE_4_S4-25A_2026-11-07.md).

## S4-28a1 one-shot Local APIC timer

Run `cargo test -p od-machine --lib local_apic::tests --offline` and
`cargo test -p od-machine --lib io_apic::tests --offline`. The Local APIC tests
cover timer MMIO through `MachineBus`, initial/current count boundaries,
divider encodings and rejection of divider changes while armed, zero/cancel/
rearm, due dispatch, masked expiry, reset and global-disable invalidation,
software-disable mask/rejection behavior, legal/illegal LVT vectors, unmasked
IOAPIC level-vector conflicts, generation exhaustion and atomic failure when
an edge cannot be posted. The IOAPIC tests verify the fallible
pair-reset API preserves the established level-route behavior.

`cargo check -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check` passed. Local APIC: 19 passed;
IOAPIC: 11 passed. Independent Sol/high review passed after fixing the software
disable behavior. This device-only slice has no vCPU clock integration,
production event pump, CPU entry, HLT wakeup or periodic mode. See the
[S4-28a1 report](../reports/STAGE_4_S4-28A1_2026-11-07.md).

## S4-28a2 shared vCPU/APIC clock

Run `cargo check -p od-machine --all-targets --offline` and
`cargo test -p od-machine --lib --offline`. The integration case creates one clock handle,
shares it between a session and Local APIC timer, advances only through the
Runnable session, and checks RDTSC, Current Count, deadline dispatch and IRR
at matching timestamps. It verifies exclusive clock advancer ownership,
APIC-local advance rejection, and that due dispatch during Paused/Stopped
leaves the event pending with `Busy`. All 190 package tests passed, including
33 diagnostic session, 19 Local APIC and 6 virtual clock tests. The all-targets
build, formatting and diff checks are recorded in
the [S4-28a2 report](../reports/STAGE_4_S4-28A2_2026-11-07.md). This proves
clock identity for this device path only; automatic dispatch, CPU delivery,
HLT progression/wakeup and production scheduling are outside this slice.

## S4-28a3 shared timer wake from HLT

Run `cargo test -p od-machine --lib diagnostic_vcpu::tests::shared_timer_advances_and_wakes_halt_wait --offline -- --exact`, then `cargo check -p od-machine --all-targets --offline` and `cargo test -p od-machine --lib --offline`. The integration case verifies `DiagnosticHaltWait` advances the shared clock only in Waiting, overflow preserves time, timer expiry posts IRR, APIC entry wakes HLT, a second due event remains pending while the wake is Entered, and `into_running` reopens dispatch. The guest handler executes IRETQ, after which RDTSC reads the time advanced during HLT. APIC/clock lock order is checked by review; all 191 package tests passed. No automatic polling, host-time source, periodic timer or production scheduler is exercised. See the [S4-28a3 report](../reports/STAGE_4_S4-28A3_2026-11-07.md).

<a id="s4-28a4-caller-driven-periodic-apic-timer"></a>

## S4-28a4 caller-driven periodic APIC timer

Run `cargo test -p od-machine --lib local_apic::tests --offline`, then
`cargo check -p od-machine --all-targets --offline` and
`cargo test -p od-machine --lib --offline`. Verify the initial expiration
boundary, periodic count wrap, fractional interval phase and next deadline
after an overdue dispatch. A large virtual-time advance must yield at most one
IRR edge per dispatch and rearm to the first future boundary. Masked expiry
must rearm without IRR; unmasking must not replay it. Verify repeated periods
while the vector is in ISR and after EOI, zero-count cancel, reprogram, reset,
overflow/generation atomicity, and that a closed shared-clock HLT dispatch gate
retains the due event. `dispatch_timer` remains explicit; tests do not establish
automatic scheduling or Linux timer compatibility. See the [S4-28a4 report](../reports/STAGE_4_S4-28A4_2026-11-07.md).

## S4-25b: CPU-owned IA32_APIC_BASE enable

Run `cargo test -p od-isa --lib apic_base_tests --offline` and
`cargo test -p od-machine --lib local_apic::tests --offline -- --test-threads=1`.
The CPU profile tests bare `RDMSR`/`WRMSR` readback, exact fixed values,
retirement, reserved-bit `#GP(0)`, unsupported relocation/BSP/x2APIC values,
Ring3 privilege fault, STOP/budget/fetch exits, active STI-shadow rejection
before fetch and unrelated decoder forms remaining outside the profile. The
machine tests verify the typed write effect is applied before the helper
returns, disable reset/MMIO blocking, same-value preservation, re-enable and
SVR behavior. `cargo check -p od-isa -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check` passed. Independent
Sol/high review found two P2 boundary issues; both were fixed and confirmed
closed. See the [slice report](../reports/STAGE_4_S4-25B_2026-11-07.md).

This does not integrate the profile into `PagedVcpuSession` or default board
wiring. Direct CPU-step callers must apply a returned effect before another
guest/device operation; only `step_apic_base_msr` supplies synchronous device
application. xAPIC base relocation, x2APIC, CPU interrupt delivery, IDT/IF,
handler execution, HLT wakeup and Linux discovery are not covered.

## S4-25c: xAPIC Fixed self-IPI

Run `cargo test -p od-machine --lib local_apic::tests --offline -- --test-threads=1`.
The six focused tests include ICR readback, high-only write without dispatch,
low-write dispatch, the Fixed Self path from IRR through priority eligibility,
acknowledge/ISR and EOI, repeated same-vector commands before and after
acknowledge, TPR priority blocking, software-disable and global-disable
behavior, reset clearing both ICR halves, and rejection atomicity for invalid
delivery/shorthand/vector/trigger/level/reserved/width/alignment. Also run
`cargo check -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check`; all passed. Independent
Sol/high review found and closed one P2 issue: the read-only Delivery Status
bit is ignored on write, while it continues to read Idle; reserved bits remain
rejected. The source is Intel SDM release 093, Volume 3A,
§§13.6.1–13.6.2.3 and 13.8.3–13.8.5. See the [slice report](../reports/STAGE_4_S4-25C_2026-11-07.md).

These checks establish only local APIC device-state transitions. They do not
establish APIC bus messaging, CPU interrupt acceptance or handler execution,
IF/IDT semantics, HLT wakeup, external destinations, IOAPIC, default board
wiring or Linux boot.

## S4-25d: bounded CR8 ↔ xAPIC TPR

Run `cargo test -p od-isa --lib cr8::tests --offline` and
`cargo test -p od-machine --lib cr8 --offline -- --test-threads=1`. The focused
coverage checks exact MOV encodings, CPL and reserved-bit faults, LOCK/fetch/
STOP/budget boundaries, canonical48 fetch boundaries, APIC admission, TPR
readback and low-nibble clearing, and self-IPI vector `0x51` blocked at TPR
class 5 then made eligible at class 4 before acknowledge/EOI. It also confirms
pending/ISR persistence and failure non-retirement. Run
`cargo check -p od-isa -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check`; record the actual result
in the slice report. Independent Sol/high review must close findings before
acceptance. The profile does not test general CR8 decoding, CPU handler
delivery, `PagedVcpuSession`, JIT, default board wiring or guest boot. See the
[slice report](../reports/STAGE_4_S4-25D_2026-11-07.md).

## S4-26a: fixed-edge IOAPIC route

Run `cargo test -p od-machine --lib io_apic::tests --offline -- --test-threads=1`
and `cargo test -p od-machine --lib local_apic::tests --offline -- --test-threads=1`.
The four IOAPIC tests cover reset and selector/readback, independent route
halves, read-only status writes, masked edge loss, no dispatch on unmask or
high-to-high, new-edge vector updates, pending IRR behavior, TPR blocking,
acknowledge/EOI, disabled-destination retry, and failure atomicity. Nine Local
APIC tests provide adjacent device regression coverage. Also run
`cargo check -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check`; all passed. Independent
Sol/high review found and closed the P2 on read-only Delivery Status/Remote IRR
writes; no findings remain. The tests do not establish level delivery, other
IOAPIC routes, default board wiring, CPU handler execution or Linux boot. See
the [slice report](../reports/STAGE_4_S4-26A_2026-11-07.md).

## S4-26b: fixed-level IOAPIC route

Run `cargo test -p od-machine --lib io_apic::tests --offline -- --test-threads=1`
and `cargo test -p od-machine --lib local_apic::tests --offline -- --test-threads=1`.
The eleven IOAPIC tests cover level acceptance and TMR/Remote IRR state, TPR
eligibility, acknowledge/EOI retrigger, asserted/deasserted and masked/unmasked
transitions, nested edge compatibility, route-change conflict rejection,
disabled-destination edge retry, vector reservation while a level route is
masked/inactive, SVR-disabled assert/unmask rejection, software-disable veto,
and independent versus coordinated reset.
The reset regression verifies that each independent reset preserves the other
controller and that outstanding Remote IRR vetoes both without mutation. Ten
Local APIC tests cover adjacent register, priority, self-IPI, CR8 and APIC-base
behavior. `cargo check -p od-machine --all-targets --offline`,
`cargo fmt --all -- --check` and `git diff --check` passed. Independent
Sol/high review initially found a P2 reset-state regression; it was fixed and
the changed slice passed repeat review. Graphify and layout results are in the
[S4-26b report](../reports/STAGE_4_S4-26B_2026-11-07.md). This does not
establish guest CPU interrupt/handler delivery, board registration or Linux
discovery.

## S4-27a: caller-driven APIC interrupt entry

Run `cargo test -p od-machine --lib diagnostic_vcpu::tests --offline -- --test-threads=1`,
`cargo test -p od-machine --lib local_apic::tests --offline -- --test-threads=1`,
`cargo test -p od-machine --test diagnostic_vcpu_api --offline -- --test-threads=1`,
and `cargo test -p od-isa --lib interrupt::tests::every_stack_store_failure_keeps_cpu_state_and_prior_memory_writes --offline -- --test-threads=1`.
The end-to-end fixture configures INTIN4 as an active level route, accepts it
into the Local APIC, enters through the selected IDT gate, and successfully
retires the one-budget handler instruction that reads a RAM sentinel. It checks the exact same-CPL
five-qword frame, cleared IF, IRR→ISR transition, and Remote IRR retention until
EOI. Adjacent cases cover no eligible/masked route, IF masking, blocking STI
shadow, TPR ineligibility and a partial stack-write abort; the abort preserves
the pending APIC request, makes the session terminal and prevents retry.
`cargo check -p od-machine --all-targets --offline`, formatting and diff
validation passed. Independent Sol/high review found no remaining findings.
The caller must also certify no pending synchronous exception, debug/NMI event
or other interrupt inhibition, as required by the underlying ISA delivery
contract. A handler fault after successful entry does not roll back the entry
or APIC acknowledgement.
This does not test automatic instruction-boundary polling, IRET, guest EOI,
HLT wakeup, paged delivery or JIT. See the
[S4-27a report](../reports/STAGE_4_S4-27A_2026-11-07.md).

## S4-27b. Guest EOI, same-CPL IRETQ and resume

Run `cargo test -p od-machine --lib diagnostic_vcpu::tests --offline -- --test-threads=1`,
`cargo test -p od-machine --lib local_apic::tests --offline -- --test-threads=1`,
`cargo test -p od-machine --test diagnostic_vcpu_api --offline -- --test-threads=1`,
and the existing `od-isa` same-CPL IRETQ frame/failure tests. The 12 diagnostic
session cases include guest `MOV dword [0xFEE000B0],EAX` EOI through the
MachineBus MMIO path, the fixed IRETQ profile, and resumption of one interrupted
instruction. All 16 original RSP low-nibble alignments must restore the exact
RSP, RIP, CS, SS and RFLAGS; the resumed instruction reads a RAM sentinel.
Further cases cover deasserted and held level lines, one new request accepted
into IRR at EOI with CPU delivery deferred until the next caller boundary, and fetch/frame/CS/SS IRETQ read aborts after
EOI without CPU commit or repeat EOI; aborted sessions become terminal.
`cargo check -p od-machine --all-targets --offline`, fmt/diff, and independent
Sol/high review passed. This remains a caller-driven flat identity profile; it
does not establish paged IRETQ, automatic polling, HLT wakeup, JIT or Linux
acceptance. See the [S4-27b report](../reports/STAGE_4_S4-27B_2026-11-07.md).

## S4-27c: caller-driven STI-profile APIC quantum

Run `cargo test -p od-machine --lib diagnostic_vcpu::tests --offline -- --test-threads=1`,
`cargo check -p od-machine --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. The 18 diagnostic session tests cover a pending level
IRQ across `CLI; STI; NOP` caller quanta, selection only after the shadow
clears, TPR-blocked eligibility, the STI profile's restricted handler
instruction, delivery abort, and no-mutation admission rejection for
insufficient flag definedness, missing shadow, configured TSC, non-Running
activity, wrong CPL, paused state, CR0/CR3/CR4 and invalid Blocking+IF=0. A
partial REP iteration-budget exit must block APIC delivery and IRETQ until an
ordinary run retires the REP instruction. Successful entry remains
acknowledged if the handler attempt is unsupported. Independent Sol/high
review passed with no findings.

The caller certifies the instruction boundary and absence of pending
synchronous/debug/NMI/MOV-SS inhibition, serializes the complete CPU/APIC/IOAPIC
operation, and supplies the private synchronous RAM and ISA entry guarantees.
This method does not cover arbitrary instructions or the rest of the guest
instruction stream; it is a bounded bridge over the existing exact STI
profile. HLT wakeup, MOV SS/NMI, TSC/RDTSC, REP continuation inside the quantum,
JIT, automatic EOI/IRET and Linux acceptance remain open. See the
[S4-27c report](../reports/STAGE_4_S4-27C_2026-11-07.md).

## S4-27d: explicit HLT wait and maskable APIC wake

Run `cargo test -p od-machine --lib diagnostic_vcpu::tests --offline -- --test-threads=1`,
`cargo check -p od-machine --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. The 22 diagnostic session tests include exact bare-HLT
retirement, failed HLT profile/fetch with original session recovery, repeated
Waiting without state mutation for IF=0/no request/TPR blocking, APIC wake with
the post-HLT RIP in the exact five-qword frame, and caller-controlled handler,
guest EOI, IRETQ and post-HLT sentinel resume across all 16 RSP alignments.
Further cases cover eligibility gating for `into_running`, STOP before wake and
after committed entry, delivery abort with pending IRQ/no retry, and REP
partial boundary rejection. The post-entry STOP test confirms that committed
frame and APIC acknowledgement remain while handler execution is forbidden.
Independent Sol/high review passed after one P2 STOP-lifecycle fix.

The caller serializes CPU/APIC/IOAPIC access, supplies the private synchronous
RAM and ISA entry guarantees, and polls explicitly. No background wakeup,
asynchronous event arrival, NMI/debug/MOV-SS, timer, general scheduler, JIT or
automatic handler/EOI/IRET is implemented. See the
[S4-27d report](../reports/STAGE_4_S4-27D_2026-11-07.md).

## S4-27e: ordinary APIC quantum

Run `cargo test -p od-machine --lib diagnostic_vcpu::tests --offline -- --test-threads=1`,
`cargo check -p od-machine --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. The 29 diagnostic-session tests cover ordinary-run
parity for RFLAGS definedness and configured TSC, IF=0 and TPR-blocked pending
vectors, explicit STOP before APIC/memory, profile rejection without fallback,
entry delivery and handler terminal outcomes, frame-write abort/no retry, and
REP partial-boundary rejection. The end-to-end pending-during-handler case
posts vector B while handling vector A; B remains in IRR with IF clear, then
the next quantum after guest EOI/IRETQ enters B before fetching the interrupted
instruction. A second EOI/IRETQ resumes the original stream. Independent
Sol/high review passed after fixing STOP to persist the Stopped session state.

This is explicit one-budget interpreter polling. It does not provide automatic
arbitration inside longer runs or JIT blocks, general event-queue dispatch,
MOV-SS/NMI, HLT integration or Linux acceptance. See the
[S4-27e report](../reports/STAGE_4_S4-27E_2026-11-07.md).

## S4-27f1: bounded JIT event handoff

Run `cargo test -p od-machine --lib diagnostic_vcpu::tests --offline -- --test-threads=1`
(32 passed), `cargo test -p od-jit --lib --offline` (412 passed), and
`cargo test -p od-host --test diagnostic_jit_event --offline -- --test-threads=1`
(2 passed on ARM64). The host tests execute the actual `PublishedBlockExecutor`:
one observes an event during publication and proves no native entry; the other
executes a native prefix, yields before NOP on a persistent event, then resumes
after the owner clears it and verifies continuation through NOP and the native
suffix without re-executing the prefix. The dispatcher unit regression covers
executor `Yield(EventPending)` and zero progress counters.

The machine tests check stable full-range resume, backward branch context,
instruction-boundary admission, byte revalidation, RAM-only preflight,
overflow, cross-page and MMIO/unmapped rejection, no rejected device callback,
pre-executor profile rejection without session mutation, and no CPU commit
after dispatch error. Also run
`cargo check -p od-machine -p od-jit -p od-host --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check`. The layout checker currently
reports the pre-existing unmapped root `smoke/` directory (ISSUE-056).

This is cooperative event handoff at dispatcher boundaries. It does not
interrupt an executing native block asynchronously or deliver the event via
APIC. See the [S4-27f1 report](../reports/STAGE_4_S4-27F1_2026-11-07.md).

## S4-27f2: JIT/APIC/EOI/IRETQ resume

Run `cargo test -p od-host --test diagnostic_jit_event --offline -- --test-threads=1`
(3 passed on ARM64). The end-to-end case uses a real `PublishedBlockExecutor`:
native prefix → `EventPending` before NOP → caller posts one eligible APIC
vector → interrupt entry stores the JIT boundary → handler reads saved RIP and
ISR bit → guest Dword EOI MMIO write clears ISR → same-CPL IRETQ restores the
boundary → caller clears its event flag → the same full code range resumes
through one interpreter NOP and a native suffix without repeating the prefix.
It asserts the JIT budget/counters, selected and pending vector, saved frame,
pre/post-EOI ISR observations, RIP/RSP/RFLAGS restoration and final registers.
Pause/resume before entry preserves CPU state and the pending caller-owned
event.

Also run `cargo check -p od-host --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check`. This is synchronous
caller-driven composition at a completed boundary; it does not prove async
interruption during native execution or general scheduler/event-queue behavior.
See the [S4-27f2 report](../reports/STAGE_4_S4-27F2_2026-11-07.md).

## S4-27g: queued IRQ and JIT/interpreter parity

Run `cargo test -p od-host --test diagnostic_jit_event --offline -- --test-threads=1`
(4 passed on ARM64). The new paired scenario schedules a due IRQ and a future
event before execution. On the JIT path, the real `PublishedBlockExecutor`
returns from the native prefix and the synchronous test caller pops the due
queue entry, requests APIC and sets notification; dispatcher exits
`EventPending` before NOP. The interpreter path executes the same two-instruction
prefix, then consumes the same due entry. Both continue through APIC entry,
handler saved-RIP/ISR reads, guest EOI, IRETQ, and the same resumed suffix.

The test compares action order and complete final `LongModeState`, checks one
queue pop and one acknowledged vector, and verifies notification plus future
event survive pause/resume before entry. This validates caller-owned bounded
composition only; it does not implement a production queue-to-APIC pump or
queue-driven HLT wake. Also run `cargo check -p od-host --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check`. See the
[S4-27g report](../reports/STAGE_4_S4-27G_2026-11-07.md).

## S4-32a: reproducible flat probe

The [S4-32a report](../reports/STAGE_4_S4-32A_2026-11-05.md) records the
artifact-to-runner acceptance path. The freestanding fixture is assembled and
linked into ELF64, then converted to a fixed raw image by the active Rust
toolchain's `ld.lld` and `rust-objcopy`. The build validator checks the entry,
three PT_LOAD segments, exact page-table entries, code bytes and zero padding,
payload bytes, raw size and hashes. Two clean builds with payload
`0x1122334455667788` produced the same raw SHA-256
`e78a7af7306e31fa316f1d64332ea4bd64abf0e854883106b1933f68bef97204`.

`od_machine::load_stage4_flat_probe` accepts only the exact image layout,
copies it verbatim to GPA `0x1000`, installs the fixed Long64/CPL0 handoff and
binds the bounded COM1 polling UART. It rejects short/oversized images, aliases,
NX code and unexpected present page-table entries. `od-runner --stage4-probe`
passes the loaded image to the existing `run_paged_probe` quantum; it does not
inject an expected byte or dispatch the guest instructions itself. A real CLI
run retired `MOV`, `OUT` and `HLT`, reported the payload's low byte on serial,
and a budget of `2` returned a nonzero `budget_exhausted` result after two
retirements with the partial UART byte visible.

Focused verification passed: loader integration **4/4**, runner CLI tests
**5/5**, runner package check, formatting and diff check. The full runner
all-targets suite was started but not completed in this slice, so no aggregate
pass count is claimed. This remains a bounded interpreter probe: no Linux boot
protocol, general kernel loader, exception delivery, IRQ/APIC, scheduler,
physical hardware or full S4-32 acceptance is established.

## S4-32b: freestanding store/load probe

The [S4-32b report](../reports/STAGE_4_S4-32B_2026-11-05.md) records a second
real-artifact path after S4-32a. The builder's explicit `store-load-probe`
profile produces the eight-byte guest sequence
`MOV [RBX],RCX; MOV RAX,[RBX]; OUT DX,AL; HLT` and stores distinct initial and
store-value qwords in the image. `od-machine` reads the artifact-owned
store-value into the documented `RCX` handoff, while the runner only invokes
the existing `run_paged_probe` quantum.

The permanent `stage4_store_load_build_cli` integration test builds two
identical inputs and one different input, compares same-input raw bytes,
executes both real `flat_probe.bin` outputs through
`--stage4-store-load-probe`, and checks the full data/RAX/UART/HLT result. It
also checks budget 1/2/3 partial state, a corrupted code artifact and an
equal-data artifact rejected by the loader. The focused machine test checks
the same handoff and committed store independently, including the exact page
table A/D and target-data committed-write callback sequence. The builder's
negative test rejects an ELF/raw pair with a substituted opcode, and the CLI
rejects repeated budget options.

This slice remains bounded interpreter evidence. It does not establish stack
execution, exception delivery/repair, alias or cross-page behavior, IRQ/APIC,
HLT wakeup, JIT integration, Linux boot or full S4-32/S4-33 acceptance.

## S4-21b: bounded paged probe quantum

The [S4-21b report](../reports/STAGE_4_S4-21B_2026-11-05.md) and
[machine contract](../specs/MACHINE_CONTRACT.md#bounded-paged-probe-quantum-s4-21b)
record the accepted `PagedCpl0ProbeV1` boundary. `step_paged_probe` accepts
only `48 8B 03` (`MOV RAX,[RBX]`), `48 89 0B` (`MOV [RBX],RCX`), bare `EC`
(`IN AL,DX`), bare `EE` (`OUT DX,AL`) and bare `F4` (`HLT`). CPU admission
checks CR0/CR4; `run_paged_probe` validates CR3/EFER and uses page-aware memory.

The quantum owns a finite instruction budget and decodes once at each boundary.
STOP wins before budget exhaustion; an exhausted budget wins before fetch.
The store→load→OUT→HLT probe is checked both in one quantum and across
single-instruction quanta. STOP/resume checks that committed store/PIO effects
are not repeated. HLT retires and enters the halted state; a repeated call is a
no-op with no fetch. It is not interpreted as guest success.

Recorded verification: `od-isa` has 2,014 unit plus 25 integration tests;
`od-machine` has 116 passing tests. Workspace all-targets check, formatting,
layout and diff passed. `graphify update .` refreshed the code graph; the
docs-aware semantic extraction was not completed. The tests cover page-aware instruction
fetch/data, literal PFEC and fault records, saved RIP/access context, prior
retirements, tracked A/D bits and callback ranges, plus distinct backend and
PIO errors. No exception delivery or CR2 update is claimed. The evidence is
local Rust testing; it does not establish kernel/Linux boot, IRQ/APIC, or full
S4-21/S4-24/S4-32 acceptance. Stage 4 remains open.

## S4-32c: bounded freestanding stack probe

The `stack-probe` builder profile assembles a real ELF64 fixture and converts it
to the exact `0x9000`-byte raw image used by the S4-32b loader. Its code is
`51 58 EE F4`; the payload qword is artifact-owned at raw offset `0x8000`.
`od-machine::load_stage4_stack_probe` validates the same fixed page-table
layout, code bytes and image size, then hands the payload to `RCX`. The runner
invokes the opt-in `run_paged_stack_probe` path and reports stack qword, `RAX`,
`RSP`, retirement count, UART bytes and exit/activity as JSON.

The focused machine test checks budgets 0…4, split PUSH/POP/OUT/HLT calls,
stack canaries, qword handoff, `RAX/RSP/RIP`, `rflags_defined`, UART output and
an inert repeated call after HLT. The builder-to-runner test builds two equal
payloads and one different payload, verifies reproducibility, runs every budget,
and rejects corrupted code and page-table artifacts. Direct ISA regression
also verifies that the legacy `PagedCpl0ProbeV1` still rejects `PUSH RCX` and
`POP RAX`. Stack-fault tests cover nonpresent PUSH/POP and read-only PUSH
PFEC/CPU-commit boundaries. This is bounded local
Long64/CPL0 interpreter evidence only: stack exception delivery/repair, CR2,
cross-page transactions, IRQ/APIC, HLT wakeup, JIT, Linux, privileged hardware
and parent S4-32/S4-33 remain unclaimed until separate evidence exists.

## S4-33a: guest RAM alias

The strict `alias-probe` builder profile places a distinct initial qword and
store value in its image and maps data GVA `0x800000` plus stack GVA `0xa00000`
to GPA `0x9000`. Machine tests check that budget zero preserves the initial
value, guest store changes the shared physical qword, POP reads the new value
through the stack alias, each leaf PTE receives the expected A/D bits, and
committed callback ranges occur in exact order. Builder/runner tests compare
multiple payloads and reject an injected remapping that still reaches HLT but
does not satisfy the alias-result oracle. The manifest's `mappings` and `stack`
fields must agree. This is uncached interpreter evidence; TLB/JIT, Linux and
physical hardware claims are outside the case.

## S4-17a: paged MOV differential harness

Run the cross-component harness with:

```text
cargo test -p od-host --test stage4_memory_fault --offline -- --test-threads=1
cargo test -p od-testkit --lib --offline memory_fault::tests -- --test-threads=1
```

The integration target runs on Apple Silicon macOS, the platform supported by
`PublishedCode` executable-memory publication. On other hosts this target is
compiled out; the testkit comparator unit tests remain portable.

The deterministic Long64/CPL0 case is exact `48 8B 03` (`MOV RAX,[RBX]`),
with code linear address `0x00401000` and operand linear address
`0x8000000120` mapping to a separate physical RAM page. The harness creates an
independent `MachineBus` from the same case descriptor for the uncached
interpreter and published JIT. It checks both a successful qword load and a
non-present leaf PTE fault. Code-side page-table A bits are initialized before
execution so interpreter instruction fetch does not add effects absent from
the published JIT path; data-side A updates remain observable in physical RAM
deltas and ordered callbacks. The test verifies that each changed entry differs
by exactly the Accessed bit, and that each backend's complete #PF exit agrees
with its `PageFaultRecord`.

`od_testkit::memory_fault::MemoryFaultObservation` records the case digest,
all GPRs, RIP, RFLAGS plus its definedness mask, retirement, outcome, intended
fault CR2 value, actual architectural CR2 availability, table-byte deltas,
ordered physical write ranges, and device-event availability. This bounded
RAM-only case declares an observed empty device-event list. The page-fault
record supplies a CR2 value but these step APIs do not commit architectural
CR2, so that state is recorded unavailable. A post-execution injected
missing-A-bit result must compare unequal at its physical address and bit.
Cached interpreter execution is explicitly unsupported: the current cached
translation probe is not a cached instruction interpreter and does not carry
tracked A/D effects. This compares two local paths that share the page walker;
it is not independent hardware evidence. Reducer coverage and broader
S4-17 cases remain open. See the [S4-17a report](../reports/STAGE_4_S4-17A_2026-11-06.md).

## S4-18c: target descriptor Accessed update

Reproduce the bounded delivery checks with:

```text
cargo test -p od-isa --lib interrupt::tests:: --offline -- --test-threads=1
cargo test -p od-machine --lib paged_exception_delivery::tests --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
```

The system target covers guest `LIDT → LGDT → MOV [RBX],RCX → #PF` delivery
through the guest-loaded tables with target descriptor A=0, exact six-qword
frame, tracked GDT PTE Accessed/Dirty transitions, a one-byte descriptor update
and handler load. Negative controls cover an A=1 descriptor on a read-only GDT,
an invalid A=0 target, and a read-only A=0 update fault with its secondary PFEC,
CR2, retained frame and no CPU entry commit. A distinct-linear-address
GDT/stack physical alias is rejected before frame stores. Additional cases
make the saved RIP overwrite the GDT leaf PTE with a read-only mapping and a
redirect to another RAM page; preflight preserves the PTE and redirected
target, with no frame write committed. Another case makes an earlier frame
slot rewrite a stack PTE used by a later slot, which is rejected before any
frame write. Missing-page and physical-hole cases confirm the first stack
failure keeps its original fault/backend classification even when an
unreachable later slot aliases the descriptor. The emulator's chosen store order is not claimed as
universal hardware fault ordering; a secondary #PF would require #DF, which
remains unsupported here. This does not demonstrate recovery, restartability,
physical-hardware equivalence, or the rest of Stage 4. Results are recorded in
the [S4-18c report](../reports/STAGE_4_S4-18C_2026-11-06.md).

<a id="s4-19c-secondary-pf-double-fault-delivery-and-shutdown"></a>

## S4-19c: secondary #PF, double-fault delivery, and shutdown

Run the bounded delivery checks with:

```text
cargo test -p od-isa --lib interrupt::tests:: --offline -- --test-threads=1
cargo test -p od-machine --lib paged_exception_delivery::tests --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
```

The positive system case makes the target #PF code descriptor's Accessed-bit
write fault on a read-only GDT. It checks the original and secondary page-fault
records, exactly one vector-8 delivery, zero #DF error code, no forced RF in the
#DF frame, preserved CR2, all six frame slots, and execution of the first
handler MOV. The shutdown case faults while writing the #DF frame to an absent
stack page; it checks the newest CR2, retained frame-prefix writes, shutdown
activity, no handler step, and a subsequent paged step with zero fetches,
accesses, A/D updates, or writes. Existing IDT/GDT/stack failure cases also
check their final attempt outcomes. The focused ISA group covers #DF frame
construction and failure ordering.

This is local emulator evidence for #PF → #DF and #DF-delivery #PF → shutdown
inside the bounded profile. It does not establish other exception-pair
precedence, hardware ordering, handler repair, INVLPG, IRETQ, or retry. Results
are recorded in the [S4-19c report](../reports/STAGE_4_S4-19C_2026-11-06.md).

<a id="s4-19d-guest-pte-repair-and-invlpg"></a>

## S4-19d: guest PTE repair and INVLPG

The system scenario raises a real write-protection #PF, delivers it through
the IDTR/GDTR values set in its initial fixture state, then executes guest
`MOV [RDI],RSI` to change the selected writable alias PTE-page mapping's leaf
data PTE from read-only to read-write while retaining A and supervisor-only
U/S=0. It next executes guest `INVLPG [RBX]` for the
original faulting address. Checks cover unchanged faulting data, unchanged
RBX/RCX, CR2, retired instruction count, exact PTE value, invalidation address,
and tracked A/D and callback order. No host PTE patch occurs after the initial
fixture setup.

This stops before IRETQ and retry. Since the tracked paged path does not keep a
translation cache, the scenario demonstrates guest invalidation execution but
not stale-TLB eviction or that the invalidation is needed. Results are recorded
in the [S4-19d report](../reports/STAGE_4_S4-19D_2026-11-06.md).

<a id="s4-19e1-guest-error-code-discard-and-paged-iretq"></a>

## S4-19e1: guest error-code discard and paged IRETQ

The system scenario extends the actual #PF → guest PTE repair → INVLPG path.
Guest `LEA RSP,[RSP+8]` removes the error-code slot and guest `IRETQ` reads the
five-qword frame plus real CS/SS descriptors through the tracked Long64 walker.
It checks return to the original faulting RIP, exact restoration of the
unaligned RSP and RF=1, unchanged CR2/control registers/GPR and unchanged data.
The fixture provides a real pre-accessed writable CPL0 SS descriptor. No host
instruction patches the frame or return state.

Read page faults update CR2 and retain already completed tracked A/D effects;
backend failures remain separate. Paged IRETQ validates CS before checking
saved SS selector/bounds and does not commit partial CPU state. This stops at
the original MOV before retry. The tracked path has no translation cache.
Results are recorded in the
[S4-19e1 report](../reports/STAGE_4_S4-19E1_2026-11-06.md).

<a id="s4-19e2-faulting-mov-retry-and-data-commit"></a>

## S4-19e2: faulting MOV retry and data commit

After the guest return in S4-19e1, the exact original `MOV [RBX],RCX` is
executed again from its faulting RIP. The test checks one retirement, one
qword data write, the preceding tracked D-bit update (`0xe023 → 0xe063`), RF=0
after the retried instruction, and a following guest `MOV RAX,[RBX]` readback.
The adjacent qword canary is unchanged before and after the readback. No host
instruction patches RIP, RSP or PTE after the guest fault begins.

RF-clear tests cover successful MOV, operand #GP, data #PF/backend error and
pre-start exits. `rflags_defined` remains unchanged. This is the bounded
Long64 CPL0/WP=1 private-RAM profile; the tracked path has no translation
cache, so stale-TLB behavior is not claimed. See the
[S4-19e2 report](../reports/STAGE_4_S4-19E2_2026-11-06.md).

<a id="s4-20a-synthetic-paged-iretq-outer-return"></a>

## S4-20a: synthetic paged IRETQ outer return to CPL3

This CPU-only profile checks the outer-privilege return path for exact paged
`48 CF`. The initial state is the synthetic four-level Long64 profile with
`CR4.LA57=0`, CPUID leaf 7 zero, a CPL0 CS cache and null DS/ES/FS/GS selectors.
The guest frame contains RIP, CS, RFLAGS, RSP and SS. User CS/SS descriptors
are the bounded flat, already-Accessed GDT forms recorded in the
[machine contract](../specs/MACHINE_CONTRACT.md#synthetic-paged-iretq-outer-return-to-cpl3-s4-20a).

Unit tests verify exact CPU state after success, atomic rejection of malformed
selectors/descriptors/addresses/flags, separate guest paging faults and backend
read errors, and first-fault ordering across the frame and CS/SS descriptor
reads. Run:

```sh
cargo test -p od-isa --lib iretq::tests --offline -- --test-threads=1
```

This slice does not execute the next user instruction or prove machine
`AccessContext` derives CPL from CS; that integration is S4-20b. It does not
establish Linux boot or implement #GP/#SS classification for rejected return
state. See the [S4-20a report](../reports/STAGE_4_S4-20A_2026-11-06.md).

<a id="s4-20b-machine-iretq-and-cpl3-load"></a>

## S4-20b: machine IRETQ followed by one CPL3 load

The machine wrapper returns from CPL0 to CPL3 and executes exactly
`48 8B 03` (`MOV RAX,[RBX]`) using CR3 from CPU state and CPL derived from the
committed CS selector/cache. It does not accept caller-supplied privilege or
CR3 values. The bounded instruction fetch must fit in one guest page; offset
`0xffd` is covered, while cross-page instruction fetch is outside this slice.

System tests cover successful user fetch/data read, supervisor-only fetch and
data mappings at every page-table level, PFEC `0x15`/`0x05`, exact CR2 and saved
RIP, Accessed updates, no Dirty or operand write, a physical-hole backend abort,
and a late SS-descriptor page fault before IRETQ state commit. A second
instruction fault preserves the already completed CPL3 transition; an IRETQ
fault does not run the load. ISA profile tests reject nearby encodings without
operand reads or retirement; instruction fetch may already have occurred
during bounded decoding. The MOV clears RF before operand access, so that
defined flags effect remains if the later data access aborts.

Run the focused admission test and the complete machine system target:

```sh
cargo test -p od-isa --lib cpl3_load_ --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
```

The profile does not support cross-page instruction fetch, user stores,
exceptions delivered from CPL3, syscalls, or Linux boot. See the
[S4-20b report](../reports/STAGE_4_S4-20B_2026-11-06.md) and
[machine contract](../specs/MACHINE_CONTRACT.md#machine-iretq-followed-by-one-cpl3-load-s4-20b).

## S4-20c: paged SYSCALL → kernel store → SYSRETQ → user load

The four-step machine test exercises the exact instruction bytes through
tracked paging, with a supervisor RW mapping and a user RO alias to one data
qword. It verifies the nonce written by the kernel in CPL0 is observed by the
final CPL3 load, the CPL transitions and selector caches, RCX/R11/FMASK and
RFLAGS definedness, and unchanged RSP/CR3. STAR selector derivation, SYSRETQ's
`0x3c7fd7` flags mask, reserved-bit handling and definedness are covered by ISA
tests.

Failure tests cover SCE-disabled #UD, invalid SYSRETQ CPL/canonical address,
saved TF rejection, live TF/RF admission, page permissions and fetch/data
faults, physical backend aborts, and instruction/operand page boundaries. The
ISA test checks that saved-TF rejection occurs after required fetch with no CPU
commit. A separate machine-level probe during independent review (outside the
33-test target) confirmed the exact fetch A-bit PTE update and
`committed_write` callback on that path. Exact A/D PTE values and callback
order are checked by the system target for success, kernel-store #PF/backend
abort, and late user fetch/read failures; already committed SYSCALL state and
kernel-store writes remain visible.

Run the focused ISA, complete machine-system, and affected machine checks:

```sh
cargo test -p od-isa --lib syscall --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
cargo check -p od-machine --all-targets --offline
```

All 5 focused ISA tests and all 33 system tests passed. Formatting and the
machine all-target check passed; the build reports the existing unrelated
`od-jit::block::CodeMemory::one_instruction` dead-code warning. Graphify was
updated after code changes (27,525 nodes, 69,045 edges). Independent
Sol/high review accepted the slice with no P0–P3 findings. Cross-page accesses,
single-step/debug behavior, asynchronous events, stack switching, exception
delivery from CPL3 and Linux boot remain outside this slice. See the
[S4-20c report](../reports/STAGE_4_S4-20C_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[machine contract](../specs/MACHINE_CONTRACT.md#paged-syscall-kernel-store-sysretq-and-user-load-s4-20c).

<a id="s4-20d-paged-cpl3-mov-store-load-after-iretq"></a>

## S4-20d: paged CPL3 MOV store/load after IRETQ

The end-to-end sequence performs paged IRETQ, the accepted S4-20b user load,
then exact `48 89 0B` store and `48 8B 03` reload. The nonce is different from
initial data. Checks cover separate retirement and RIP progression, CPL3
selector caches, unchanged RSP/CR3, one operand write, and the nonce in RAX.
The legacy load-only entry still rejects the store encoding.

The system target tests user RO and supervisor-only stores with PFEC `0x07`,
supervisor-only read `0x05`, supervisor-only fetch on every page-table level
and NX fetch `0x15`, non-present instruction fetch `0x14`, and non-present
read/store `0x04`/`0x06`. It checks exact CR2/saved RIP, no retirement or
operand/D write on faults, and physical backend failure as `OperandPhysical`.
A clean first user fetch-fault fixture independently checks expected A-bit PTE
values and ordered callback ranges. Exact A/D readbacks and callback order are
also checked for non-present data and store-fault paths. Supervisor-only read
and late non-present fetch cases verify fault records and retention of earlier
committed effects without a separate exact A/D/callback assertion.

Run the focused ISA group, full machine-system target and affected machine
check:

```sh
cargo test -p od-isa --lib cpl3_ --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
cargo check -p od-machine --all-targets --offline
```

All 9 matching ISA tests and 35 system tests passed, as did formatting, the
machine all-target check and `git diff --check`. Graphify was updated after
code changes (27,540 nodes, 69,099 edges). Independent Sol/high review accepted
the slice with no P0–P3 findings. Live TF/RF/AC, cross-page accesses, privileged
instructions, invalid-selector IRETQ and CPL3 exception delivery remain outside
this slice. See the [S4-20d report](../reports/STAGE_4_S4-20D_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[machine contract](../specs/MACHINE_CONTRACT.md#paged-cpl3-mov-store-load-after-iretq-s4-20d).

<a id="s4-20e-paged-cpl3-mov-cr0-after-iretq"></a>

## S4-20e: paged CPL3 MOV CR0 after IRETQ

`cpl3_mov_cr0_first_fetch_enforces_user_page_permissions` executes the new
`step_paged_cpl3_mov_cr0` entry as the first user instruction after paged
IRETQ. The matrix covers supervisor-only fetch at each paging level, NX, and a
non-present leaf. For the non-present case it asserts `PFEC=0x14`, linear
address/CR2 and saved RIP equal to the user instruction address, zero
retirement, and unchanged CPU state except CR2. It checks the three completed
A-bit updates, exact readbacks of the three present ancestor entries, and
ordered write callbacks; no D-bit update or operand write occurs. The
non-present leaf is not read back after the fault.

`cpl3_mov_cr0_faults_without_committing_cpu_state` checks `#GP(0)` for the
bare instruction for varied RAX and IOPL values, and `#UD` for the exact LOCK
form. It verifies no retirement, no partial CPU commit, and preservation of
the CR2 sentinel. The same fixture checks rejection before fetch/callbacks for
invalid CS, CR0/CR4, TF/RF/AC and active STI shadow, and checks an unlisted
instruction without operand access. `cpl3_mov_cr0_instruction_offset_boundary_is_exact`
checks `0xff1` acceptance and `0xff2` pre-fetch rejection.
`cpl3_mov_cr0_physical_fetch_failure_keeps_walk_effects_without_retirement`
checks `OperandPhysical` at GPA `0x30000` and retention of successful earlier
A updates. ISA fetch-fault tests inject a fault at each byte of bare and LOCK
encodings to verify fetch precedence over #GP/#UD. The page-walk test
`fetch_accessed_write_failures_are_tracked_before_physical_fetch` injects
`TableWrite` failures at each A-bit level through the internal writer seam;
this validates the shared walker, not the machine-entry report projection.

The established S4-20e evidence includes 10 `cpl3_` ISA tests and 39 system
test functions, with one additional non-present-fetch assertion case added
inside the system suite. The targeted first-fetch test also passed separately.
The page-walk A-write matrix and its new fetch-specific case passed, as did
`cargo check -p od-machine --all-targets --offline`, formatting and
`git diff --check`. Graphify was updated after the code changes in the primary
checkout (27,555 nodes, 69,202 edges). Sol review identified that non-present PFEC `0x14` needed to
exercise the new entry; the direct system case was added and its targeted test
passed. Independent Sol code and documentation reviews passed with no
remaining findings. See the
[S4-20e report](../reports/STAGE_4_S4-20E_2026-11-07.md),
[machine contract](../specs/MACHINE_CONTRACT.md#paged-cpl3-mov-cr0-after-iretq-s4-20e),
and [plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты).

## S4-21c bounded paged vCPU owner

`cargo test -p od-machine --lib --offline` passed 120 unit tests, including the
four `paged_vcpu_session` tests. `cargo test -p od-machine --test
paged_probe_quantum --test uart_rx_events --offline` passed 21 paged-quantum
and 7 UART RX tests. They cover STOP before fetch and between instructions,
pause/resume, HLT, fixed-image reset from paused/stopped/halted/faulted, stale
generation, pending due/future RX, replacement UART and invalid-image
atomicity. The store/OUT case patches code through private test access after
loading; the public fixed image remains load/OUT/HLT. Generation exhaustion is
covered by the existing EventQueue unit test, not injected through this owner.
Independent Sol/high review found no P0–P3 issues. No runner, IRQ or hardware
reset acceptance is claimed. See [report](../reports/STAGE_4_S4-21C_2026-11-07.md).

## S4-21d runner completion and cancellation

`cargo test -p od-runner --test stage4_probe_cli --offline -- --test-threads=1`
passed 8 tests; the corresponding `stage4_probe_build_cli` command passed 1.
They cover exact nonce and byte completion, absent and mismatched expectation,
STOP before fetch and after load and OUT, work limit, malformed arguments and
loader refusal. The `PagedVcpuSession::drain_tx` unit test passed, as did
`cargo check -p od-runner --all-targets --offline`, workspace formatting and
diff checks. A separate runner unit test checks error category mapping. The
fixed loader does not expose fault injection through the CLI. Independent
Sol/high review passed. See [report](../reports/STAGE_4_S4-21D_2026-11-07.md).

## S4-22b Paged session clock and UART RX

`cargo test -p od-machine --lib --offline` passed 123 unit tests. The new
`clock_drives_rx_and_split_advances_match` test verifies that RX dispatch reads
the session clock, advancement is explicit, and split advancement matches a
single advancement. `clock_guards_halt_reset_and_overflow` checks zero frequency,
pause/STOP/terminal guards, active guest HLT progression without RX dispatch,
reset time preservation, and atomic time/TSC overflow. The run and polling path
does not move time. Stage 4 runner CLI tests passed 8 cases and the artifact
build CLI passed 1; runner all-targets check, formatting, and diff checks also
passed. The independent Sol/high review found no code findings. The runner's
frequency is diagnostic and is not advertised to the guest. See [report](../reports/STAGE_4_S4-22B_2026-11-07.md).
## S4-30a PCI ECAM window and type-0 function model

45. S4-30a checks the ECAM window decode, width/alignment errors, absent-function
all-ones reads and ignored writes, a `#[cfg(test)]` diagnostic type-0 endpoint
(read-only fields, Command mask, BAR0 sizing) and MMIO dispatch through
`MachineBus`: `cargo test -p od-machine pci_ecam`. BAR-to-MMIO mapping and
MCFG are not covered. See [report](../reports/STAGE_4_S4-30A_2026-11-08.md).

## S4-30b Legacy conf1

46. S4-30b checks legacy conf1 ports 0xCF8..0xCFF over the same function table as
ECAM, always through `MachineBus::read_port`/`write_port` with a real
`PortRegionBinding`: the Linux `pci_check_type1` sequence (outb 0xCFB, outl
0x80000000 to 0xCF8, inl 0xCF8 reads it back, latch restored), latch bit mask
`0x80FF_FFFC`, bit31=0 and bus>0 giving all ones with dropped writes,
bus/device/function/register decode, byte and word access through 0xCFC+n,
rejection of qword and misaligned accesses with no state change, byte/word
access to 0xCF8 (ignored writes, all-ones reads), explicit behaviour of 0xCF9
(reset control not implemented: `UnknownOffset`) and 0xCFA/0xCFB (writes
ignored, all-ones reads), the Linux conf2 probe being rejected for any latch
state, and equivalence with ECAM for BAR0 sizing and Command in both
directions. The bus>0 difference (absent through conf1, `UnknownOffset` through
ECAM) is a test. `cargo test -p od-machine pci_ecam`. Host bridge, MCFG, BAR
mapping, conf2 and reset control are not covered. See
[report](../reports/STAGE_4_S4-30B_2026-11-08.md).

## S4-30c ECAM на плате (opt-in)

47. S4-30c checks the board-level registration of the ECAM window and conf1 ports
behind `FirmwareBoardConfig::pci_ecam` (off by default): the default handoff stays
byte for byte identical to the golden reference with code 7 `PRESENT = 0`; with the
flag code 7 is `PRESENT = 1` (base `0xFC10_0000`, length `0x10_0000`, segment 0,
buses 0-0) inside handoff v1.0 with one extra MMIO region owned by entry 6; the
window is a non-RAM `Mmio` region disjoint from RAM, ROM, both framebuffers, display
registers, NVMe, IOAPIC and LAPIC, below 4 GiB and inside physical36;
`patch_physical_ram` on the window is rejected; all 32 devices of the empty bus 0
read all ones through `MachineBus::read_physical`; the Linux type1 probe passes and
type2 is rejected through `read_port`/`write_port` on the board instance; a conf1 write
is visible through ECAM on a `#[cfg(test)]` diagnostic endpoint. Firmware side:
`cd firmware/uefi && cargo test` includes the exact 48 bytes of entry 7 being accepted
by the DSDT builder (`PCIE_ECAM`). `cargo test -p od-machine firmware_board` and
`cargo test -p od-machine pci_ecam`. MCFG, E820, host bridge, functions and BAR
mapping are not covered. See [report](../reports/STAGE_4_S4-30C_2026-11-08.md).

## S7-14a ACPI fixed hardware

48. S7-14a checks the ACPI PM block (`0x0600..0x0610`, opt-in `FirmwareBoardConfig::acpi_pm`, off by default). Unit tests per register in `acpi_pm.rs`: `PM1_CNT.SCI_EN` is 1 from reset and cannot be cleared; `SLP_TYP`/`BM_RLD` are stored and `GBL_RLS`/`SLP_EN` read zero; `SLP_TYP = 5` with `SLP_EN` (word write, or high-byte write alone) latches `Shutdown`, another `SLP_TYP` does nothing; `RESET_VALUE = 0x06` in the one-byte reset register latches `Reset`, other values do nothing, reads are `WriteOnly`, wider writes `CrossesRegister`; S5 and reset give different reasons and the first stays; `PM1_STS` is write-one-to-clear, `PM1_EN` keeps only defined bits; the PM timer is `floor(ns * 3579545 / 1e9)` of the virtual clock, 24 bits wide, monotonic, read-only, and two reads after a clock shift differ by the expected ticks; space/width/alignment/crossing/gap errors leave no side effect. Through a real `PortRegionBinding` on `MachineBus`: `read_port`/`write_port` reach every register, device errors surface as `BusAccessError::Device`, ports past the block are `UnmappedPort`. Board tests (`firmware_board.rs`): the handoff with and without the flag is identical and equals the golden reference; without the flag the ports are `UnmappedPort`; the block does not overlap COM1 and conf1 on a board with every optional device, COM1 and conf1 still answer, `0xCF9` is still `UnknownOffset` and does not stop the machine; the memory map is identical with the flag; guest `OUT` code run through `run_boot` stops after the `OUT` with `PagedProbeExit::Stopped` and `stop_reason` `Shutdown` or `Reset`, a stop on the last budgeted instruction is not `BudgetExhausted`, a second `run_boot` retires nothing; `run_boot` moves the clock by the retired instructions and `PM_TMR` sees it after the run (the timer is frozen inside a quantum, ISSUE-067). `cargo test -p od-machine acpi_pm` and `cargo test -p od-machine firmware_board`. FADT, MADT/MCFG, SCI routing, GPE, reset operation (S7-26) and a Linux guest (S4-38) are not covered. See [report](../reports/STAGE_4_S7-14A_2026-11-08.md).

## S4-31a MCFG

49. S4-31a checks the MCFG serializer in the firmware crate (`firmware/uefi/src/acpi/mcfg.rs`, host tests only; `od-machine` is untouched). The 48-byte PCIe ECAM entry as the board emits it with `pci_ecam` (base `0xFC10_0000`, length `0x10_0000`, segment 0, buses 0-0) gives a 60-byte table that equals a reference written by hand field by field (signature `MCFG`, length 60, revision 1, OEM ID `OPNDSK`, OEM table ID `ODFWMCFG`, 8 reserved bytes, base, segment, start/end bus, 4 reserved bytes, checksum `0x4C` computed independently); all bytes sum to zero modulo 256; a dirty output buffer does not leak into reserved bytes and bytes past the table are untouched; two calls and two buffer histories give identical bytes. Rejections: no ECAM (`PRESENT = 0`, also through `BoardManifest`), `start_bus > end_bus`, another device type, a PIO window, zero length or any length other than 1 MiB for buses 0-0, reserved `param1` bits, segment above 16 bits, zero or unaligned base, a window ending above 4 GiB (Linux `acpi_mcfg_valid_entry` would drop it without SMBIOS), segment 1 and buses 0-1 (outside the stage-4 contract), a short output buffer (no write). The validator reports bad signature, length, checksum, and an entry area that is empty or not a whole number of 16-byte structures. `cd firmware/uefi && cargo test --offline` (90 passed, 12 of them `acpi::mcfg`). XSDT/RSDP, FADT, FACS, MADT, the DSDT with PNP0C02, blob assembly, placement and a Linux guest (S4-38) are not covered. See [report](../reports/STAGE_4_S4-31A_2026-11-08.md).

## S4-31b0 const ACPI

50. S4-31b0 checks that the ACPI serializers of the firmware crate are `const fn` and write the same bytes as before (host tests only, `cd firmware/uefi && cargo test --offline`; `od-machine` is untouched). Rewritten without changing a byte: the AML layer (`aml/{encoder,names,resource,builder,validate}.rs`), the table helpers (`write_header`, `finish_checksum`, `validate_table`, `byte_sum`), `build_dsdt` / `validate_dsdt` and `build_mcfg` / `build_mcfg_from_entry` / `validate_mcfg`; the manifest accessors `BoardManifest::device`, `BoardManifest::from_entries`, `DeviceEntry::is_present`, `DeviceEntry::is_pio` became `const fn` only because the serializers take the manifest. The closure form of the AML builder (`scope`, `device`, `method`, `package`, `name`, ...) is kept for the existing tests and is wrapped around the new explicit `begin_*` / `end` pair, which is the `const fn` form; one implementation writes both. Gate: all 90 earlier tests pass with `acpi/aml/tests.rs` and `acpi/dsdt/tests.rs` byte-identical to the previous commit (golden DSDT: both devices, COM2 level/active-low, empty; every AML construct) and the inline tests of `mcfg.rs`, `table.rs`, `manifest.rs` unchanged. The hand-written 60-byte MCFG reference is the golden for the const form: `build_mcfg_from_entry` evaluated in a `const` item and `build_mcfg` (const `BoardManifest::from_entries`) in a `static` both equal it byte for byte, equal the run-time call, and `validate_mcfg` accepts the const table in a const assertion. `build_dsdt` evaluated in a `const` item (manifest with COM1 and a PCIe root with buses 0-0xFF) equals the run-time call and the golden reference of the DSDT test byte for byte (length `0xB0`, checksum `0x0E`). `cargo test --offline` gives 94 passed (90 before, +2 `acpi::mcfg`, +2 `acpi::dsdt::const_tests`); `cargo fmt --check` clean; `cargo clippy --lib --tests` has only the one warning that existed before (`too_many_arguments` in `dsdt/tests.rs`); `cargo check --offline --target x86_64-unknown-none` clean. Not covered: FADT, FACS, MADT, RSDP, XSDT, blob assembly and placement, manifest/handoff extension, PNP0C02 in the DSDT (S4-31b), a Linux guest (S4-38). See [report](../reports/STAGE_4_S4-31B0_2026-11-08.md).

## S4-31b DSDT для ECAM

51. S4-31b проверяет DSDT для ECAM (только host-тесты прошивки, `cd firmware/uefi && cargo test --offline`; `od-machine` не тронут). Один `build_dsdt` (const fn): если запись PCIE_ECAM `PRESENT`, она проходит те же проверки, что MCFG (общий `EcamWindow::from_entry`: окно в памяти, длина `(end_bus - start_bus + 1) << 20`, ненулевая база, кратная 1 MiB, конец не выше 4 GiB, сегмент 0, шины 0..=0, биты `param1`; ошибка — `DsdtError::Ecam(McfgError)`, прежний `BadPcieEcam` удалён); `\_SB.PCI0._CRS` = `WordBusNumber 0..0` + `IO(Decode16, 0xCF8, 0xCF8, 1, 8)`; после PCI0 выводится `\_SB.ECAM` (`_HID` PNP0C02, `_UID` 0, `_CRS` = `Memory32Fixed(ReadWrite, base, 0x10_0000)`); затем COM1. Без записи ECAM таблица прежняя (нет PCI0, IO и PNP0C02). Golden `reference_both` (S7-12) заменён: прежний эталон описывал запись вне контракта платы S4-30c (база `0xC000_0000`, 256 MiB, шины 0..=0xFF; длина `0xB0`, сумма `0x0E`), новая проверка MCFG его отвергает. Новый эталон — плата: база `0xFC10_0000`, 1 MiB, шины 0..=0, с COM1: 230 байт (`0xE6`), контрольная сумма `0xFE`; без COM1 — 179 байт (`0xB3`), сумма `0xD9`. Эталоны собраны вручную по грамматике AML (DefDevice, PkgLength, DefName, Buffer, Memory32Fixed, IO, WordBusNumber, EISA-id) и независимо пересчитаны отдельным Python-скриптом, который вычисляет PkgLength и суммы из частей и не делит код с Rust (совпадение побайтно, скрипт не сохранён); побайтное сравнение в run-time и в `const`, `validate_dsdt` принимает, длина и сумма проверены отдельно. Golden COM2-only и empty и все AML-тесты не менялись. Сохранены вариационные и отрицательные тесты на `build_dsdt`: байты `_CRS` PCI0 (ровно один IO 0xCF8), байты PNP0C02, сдвиг базы, окно ровно до 4 GiB допустимо, отказы с причиной MCFG (PIO, неверная и нулевая длина, база выше 4 GiB, нулевая и невыровненная база, сегмент выше 16 бит и сегмент 1, обратный порядок шин, шины вне 0..=0, биты `param1`), отсутствие PCI0/IO/PNP0C02 без записи ECAM. `cargo test --offline`: 99 passed (94 до среза; заменено 2 теста: `pci_entries_that_cannot_be_described_are_rejected` и `bus_range_reaches_bbn_seg_and_crs`, добавлено 5 новых в `acpi/dsdt/tests.rs` и `dsdt/const_tests.rs`); `cargo fmt --check` чисто; `cargo clippy --lib --tests` — только прежнее `too_many_arguments`; `cargo check --target x86_64-unknown-none` чисто. Не покрыто: FADT, FACS, `\_S5`, MADT, RSDP, XSDT, сборка blob, ресурсы PM-блока, окна MMIO PCI (aperture нет), manifest/handoff, фактическое принятие ECAM Linux (S4-38). См. [отчёт](../reports/STAGE_4_S4-31B_2026-11-08.md).

## S4-31c расширение manifest

52. S4-31c проверяет расширение manifest под opt-in флагом `FirmwareBoardConfig::acpi_tables` (по умолчанию выключен; требует `acpi_pm`, иначе `FirmwareBoardError::Config`). Без флага handoff остаётся прежним байт в байт: golden-тест `handoff_block_is_byte_identical_to_the_reference` не менялся, и с `acpi_pm` один handoff равен эталону, с `pci_ecam` + `acpi_pm` и с NVMe версия и число записей прежние. С флагом (образ с `handoff_minor_max ≥ 2`, иначе `ImageHeader`): `version_minor = 2`, `device_count` 8 (9 с NVMe), `region_count` +3, `total_size` +144, CRC верный; запись кода 9 равна 48 байтам, написанным вручную (`PRESENT|PIO`, порт `0x0600`, длина `0x10`, GSI 9, `irq_flags = 0b11`, `param0 = 0x060C_0608_0604_0600`, `param1 = 0x0506`) и совпадает с `ACPI_PM_DESCRIPTOR`; семь записей v1.0 не меняются; таблица регионов равна прежней с первой RAM-частью, разрезанной на RAM `[0, 0xE_0000)`, `ACPI_TABLES` `[0xE_0000, 0xE_F000)`, `ACPI_NVS` `[0xE_F000, 0xF_0000)` и RAM `[0xF_0000, 0x10_0000)` (регион 11 проверен по 32 байтам вручную), сумма длин не меняется, RAM-части не накрывают новые диапазоны. Карта платы (`MachineMemoryMap`) и число boot page tables с флагом и без одинаковы при `R = 128 MiB` и `R = 0xF_F400_0000` на полностью заполненной плате; область лежит внутри единственной нижней RAM и не пересекается с ROM, ECAM, NVMe, IOAPIC, LAPIC, регистрами дисплея, обоими framebuffer и верхней RAM по реальным константам; читается нулями; ниже handoff. С `pci_ecam` + NVMe + второй экран + `acpi_pm`: индексы 6/7/8 = коды 7/8/9, `owner_device` ECAM = 6, NVMe = 7, framebuffer = дисплей, область таблиц без владельца; без NVMe запись 9 получает индекс 7; PM-порт, ECAM и NVMe продолжают отвечать. Прошивка: `cd firmware/uefi && cargo test --offline` (`acpi::manifest`) — `AcpiPmEntry::from_entry` разбирает те же 48 байт (в том числе в `const`), отвергает неверный тип, флаги, длину, базу, GSI, `irq_flags`, биты `param1`, `SLP_TYP > 7` и регистры вне блока/не по порядку; `BoardManifest` на семь записей код 9 по-прежнему не принимает. `cargo test -p od-machine firmware_board`. Не покрыто: сами таблицы (FADT, FACS, MADT, RSDP, XSDT), blob, E820 (S4-37), SCI-доставка, разбор всего handoff-блока в прошивке (S7-04). См. [отчёт](../reports/STAGE_4_S4-31C_2026-11-08.md).

## S4-31d путь сборки blob

53. S4-31d проверяет путь сборки blob ACPI-таблиц и machine-side валидатор. Blob собирается без исполнения прошивки: отдельная freestanding-цель `od-acpi-blob` (`firmware/uefi/src/acpi_blob_main.rs`, `x86_64-unknown-none`, свой `acpi_blob.ld`) кладёт в секцию `.acpi_blob` статический массив, который компилятор вычисляет из `const fn` (`build_dsdt`, `build_mcfg`, заголовок blob в `acpi/blob.rs`). `tools/build/build_stage4_acpi_blob.py` только запускает `cargo build`, вырезает секцию по section header ELF и читает заголовок (version, manifest hash, load range, RSDP address, каталог таблиц); сериализатора в скрипте нет. Формат — шапка 0x60 байт (magic `ODACPIB1`, версия 1, SHA-256 manifest, `load_base` 0xE_0000, `payload_offset/len`, `rsdp_address` = 0, число таблиц, полная длина), каталог по 16 байт, payload: первые 0x40 байт — пустой слот RSDP, затем DSDT и MCFG по 16-выравниванию, промежутки нулевые. Manifest hash = SHA-256(`ODACPI-MANIFEST1` ‖ 48-байтовые записи таблицы устройств в порядке кода типа); фирменная версия — `const fn` (`acpi/sha256.rs`), машинная — `od_machine::acpi_manifest_hash` / `FirmwareBoardConfig::acpi_manifest_hash()`. Проверки. Прошивка (`cd firmware/uefi && cargo test --offline`): SHA-256 по векторам FIPS (в том числе миллион `a`) и в `const`; `const`-blob равен blob, построенному во время выполнения, побайтно; байты DSDT и MCFG внутри blob равны вызовам `build_dsdt`/`build_mcfg` для той же конфигурации; заголовок читается по зашитым смещениям; сумма каждой таблицы равна нулю, поля длины верны, выравнивание, нулевые промежутки; результат не зависит от того, чем был заполнен буфер (0x00/0xFF/0xA5); manifest hash равен SHA-256 из тега и записей по порядку типов и не зависит от порядка таблицы; любая правка base/length/gsi/irq_flags/param0/param1 COM1 или PM-записи меняет hash, сдвинутое окно ECAM меняет и hash, и байты MCFG; неизвестный/повторный тип, отсутствие записи PM, малый буфер, неверная запись PM отвергаются; без ECAM blob содержит один DSDT; зафиксированный fixture `tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin` равен blob времени выполнения. Packaging (`python3 -m unittest tools/build/test_build_stage4_acpi_blob.py`): две чистые сборки в разные `CARGO_TARGET_DIR` дают одинаковые байты секции, одинаковый `blob_sha256` и даже одинаковый ELF; секция равна fixture; metadata содержит version, manifest hash, load range 0xE_0000.., RSDP 0, таблицы DSDT/MCFG; порча контрольной суммы, magic, версии, длины, усечение, чужой ELF, отсутствующая секция и выход `e_shoff` за файл отвергаются. Машина (`cargo test -p od-machine --lib acpi_blob`): fixture принимается с hash, посчитанным из реальной конфигурации платы (`pci_ecam`+`acpi_pm`+`acpi_tables`) — это сверка таблицы устройств прошивки с `board_devices`; MCFG в blob описывает `PCI_ECAM_DESCRIPTOR`; чужой manifest (плата без ECAM, плата с NVMe) и изменение manifest без пересборки (бит hash) — `ManifestMismatch`; перекрытие двух таблиц (включая обратный порядок каталога) — `TableOverlap`; payload ниже, выше региона `[0xE_0000, 0xF_0000)`, с переходом через `0xF_0000` и с переполнением u64 — `OutOfRegion`, заход в NVS-часть `>= 0xE_F000` — `PayloadIntoNvs`, граница ровно `0xE_F000` принимается; неверная контрольная сумма, расхождение длины в заголовке таблицы и в каталоге, длина за payload, таблица короче заголовка SDT, неверная полная длина и `payload_len`, усечение; чужая сигнатура (FACP), повтор, несовпадение сигнатуры каталога и таблицы, невыровненная таблица и `load_base`, таблица в слоте RSDP, ненулевой RSDP, ненулевые зарезервированные поля и ненулевой промежуток; инверсия любого бита в полях заголовка и в любом байте любой таблицы fixture отвергается; SHA-256 машины по тем же векторам FIPS. Дефолтный handoff не менялся: golden `handoff_block_is_byte_identical_to_the_reference` и остальные тесты `firmware_board` без правок ожиданий. Не покрыто: FADT/FACS/MADT/RSDP/XSDT (S4-31e..g), разметка blob полным набором таблиц, loader и размещение в RAM, E820 (S4-37); проверка содержимого DSDT/MCFG сверх контрольной суммы и длины (содержимое сверено с ECAM-дескриптором платы только для MCFG); фактическое обнаружение Linux (S4-38). См. [отчёт](../reports/STAGE_4_S4-31D_2026-11-08.md).

## S4-31e FADT, FACS, _S5

54. S4-31e проверяет FADT ревизии 6 (без `HW_REDUCED_ACPI`), FACS и пакет `\_S5` (только host-тесты прошивки, `cd firmware/uefi && cargo test --offline`; `od-machine` и корень не тронуты). `firmware/uefi/src/acpi/fadt.rs`: `build_fadt(&AcpiPmEntry, &FadtAddresses { facs, dsdt }, out)` (`const fn`, 276 байт, сигнатура `FACP`, OEM-поля `OPNDSK`/`ODFWFACP`/`ODFW`), `build_facs(out)` (64 байта, `FACS`, версия 2, остальное 0, без checksum), `validate_fadt`/`validate_facs`. Эталоны FADT и FACS собраны вручную по смещениям полей (независимым Python-скриптом; checksum `0x6B` дополнительно просуммирован вручную) и сверены побайтно: Length `0x114`, PM1a_EVT/CNT/TMR `0x600/0x604/0x608` и длины 4/2/4, `RESET_REG` (SystemIO, 8 бит, байтовый доступ, адрес `0x60C`) и `RESET_VALUE = 6`, `SCI_INT = 9`, `Preferred_PM_Profile = 1` (Desktop), `IAPC_BOOT_ARCH = 0x24` (`VGA_NOT_PRESENT`, `CMOS_RTC_NOT_PRESENT`; `8042 = 0`), `Flags = 0x470` (`PWR_BUTTON`, `SLP_BUTTON`, `FIX_RTC`, `RESET_REG_SUP`; `TMR_VAL_EXT = 0`, `HW_REDUCED_ACPI = 0`), нули в SMI/PM1b/PM2/GPE, `X_FIRMWARE_CTRL`/`X_DSDT` равны 32-битным полям, `X_PM1a_EVT/CNT`, `X_PM_TMR` равны 32-битным блокам. Адреса таблиц передаются параметрами: DSDT `0xE_0040` берётся из каталога blob S4-31d (тест `fixture_layout_gives_the_dsdt_address` читает его из реального blob), FACS `0xE_F000` — решение S4-31c; вне региона `[0xE_0000, 0xE_F000)` для DSDT и вне 64-выровненного слота NVS для FACS сборка отказывает, не записав ни байта. `\_S5`: `build_dsdt_with_s5(manifest, slp_typ, out)` — отдельный вариант, `Name(\_S5, Package(){5,5,0,0})` = `08 5C 5F 53 35 5F 12 08 04 0A 05 0A 05 00 00` (15 байт после области `\_SB`); эталон ECAM-only DSDT с `\_S5` (`0xC2` байт, checksum `0xE4`) собран вручную. Прежний `build_dsdt` не менялся: golden DSDT (`reference_both`, `reference_ecam_only`) и все прежние 114 тестов проходят без правок. Валидаторы отвергают плохие сигнатуру, Length, checksum, ревизию, `HW_REDUCED_ACPI`, расхождение 32/64-битных полей, неверные версию/резерв FACS. Результат: 129 тестов прошивки (114 прежних + 15), `cargo fmt --check` и `cargo check --target x86_64-unknown-none` чистые. Не покрыто: MADT (S4-31f), RSDP/XSDT и включение FADT/FACS/`\_S5` в blob (S4-31g), E820 и loader (S4-37), SCI-доставка и события PM1, фактическое принятие FADT Linux (S4-38); текст ACPI 6.6 не читался (6.4 через Context7). См. [отчёт](../reports/STAGE_4_S4-31E_2026-11-08.md).

## S4-31f MADT

55. S4-31f проверяет MADT ревизии 5 для платы с одним CPU (только host-тесты прошивки, `cd firmware/uefi && cargo test --offline`; `od-machine`, корень, blob S4-31d и `docs/plans/STAGE_4.md` не тронуты). `firmware/uefi/src/acpi/madt.rs`: `build_madt(&BoardManifest, &AcpiPmEntry, out)` (`const fn`, 84 байта, сигнатура `APIC`, OEM-поля `OPNDSK`/`ODFWAPIC`/`ODFW`) и `validate_madt`. Состав: Local APIC address `0xFEE00000` (из записи кода 2), Flags = 1 (`PCAT_COMPAT`); один Processor Local APIC (тип 0, длина 8, UID 0, APIC ID 0, Enabled); один I/O APIC (тип 1, длина 12, ID 1, адрес `0xFEC00000` и GSI base 0 из записи кода 3); два Interrupt Source Override (тип 2, длина 10): IRQ 0 -> GSI 2 с flags `0x0000` (ISA: edge, active high) и IRQ 9 -> GSI 9 с flags `0x000F` (уровень `0b11 << 2`, active low `0b11`, MPS INTI flags, ACPI 6.4 Table 5.26; значение `0x000D` из текста задачи — уровень и active HIGH, поэтому не взято). Нет NMI, x2APIC и Local APIC address override. Эталон собран вручную по раскладке независимым Python-скриптом (`python3 -I`) и сверен побайтно; checksum `0x4D` просуммирован отдельно (сумма остальных байт 0xAB3, остаток `0xB3`). Проверки: поля по зашитым смещениям; список структур ровно `[0, 1, 2, 2]`; кодировка INTI flags и все четыре комбинации level/polarity; другие значения манифеста и SCI (Local APIC, I/O APIC, GSI base 1, SCI GSI 10 edge/high) двигают поля; два вызова и буфер, заполненный `0x00/0xA5/0xFF`, дают одинаковые байты; `const`-результат равен эталону; валидатор отвергает плохие сигнатуру, Length, checksum, ревизию (0, 3, 4, 6), флаги, нулевой/невыровненный Local APIC address, GSI base, не покрывающий GSI 2 (3) или GSI SCI (24), переполнение base + 24, override с bus != 0, source > 15, повтором source, неверным GSI IRQ 0, без IRQ 0, с зарезервированными INTI flags, неизвестный тип (NMI 3, x2APIC 9), нулевую и неверную длину структуры, второй Local APIC и второй I/O APIC, отсутствие каждого, усечённую структуру; сборка отказывает без записи в буфер: нет/PIO/невыровненное/не 4 KiB окно APIC, GSI base 3 и выше 32 бит, SCI GSI > 15, SCI на IRQ 0 или GSI 2, малый буфер. Результат: 143 теста прошивки (129 прежних без правок + 14), `cargo fmt --check` и `cargo check --target x86_64-unknown-none` чистые. Не покрыто: включение MADT в blob, RSDP/XSDT (S4-31g), E820 и loader (S4-37), маршрут SCI/GSI 9 в модели `IoApic` (S4-26a даёт только INTIN4), число входов IOAPIC и его ID (не из манифеста, константы 24 и 1), принятие MADT Linux (S4-38); текст ACPI 6.6 не читался, раскладки типов 0 и 1 не вернулись из Context7 и закреплены только эталоном. См. [отчёт](../reports/STAGE_4_S4-31F_2026-11-08.md).

## S4-31g RSDP, XSDT, полный blob

56. S4-31g проверяет RSDP ревизии 2, XSDT и полный blob версии 2 (прошивка: `cd firmware/uefi && cargo test --offline`; машина: `cargo test -p od-machine --lib acpi_blob`; упаковка: `python3 -m unittest tools/build/test_build_stage4_acpi_blob.py`). `firmware/uefi/src/acpi/rsdp.rs`: `build_rsdp`/`validate_rsdp` (36 байт, `RsdtAddress = 0`, две суммы) и `build_xsdt`/`validate_xsdt` (FADT, MADT, MCFG), все `const fn`; эталоны собраны независимым Python-скриптом (суммы RSDP 0x10 и 0x4D, XSDT 0x6C). Раскладка blob: RSDP на 0xE_0000 (слот 0x40), DSDT с `\_S5` на 0xE_0040, MADT 0xE_0140, FADT 0xE_01A0, MCFG 0xE_02C0, XSDT 0xE_0300, payload до 0xE_0340; FACS — отдельный 64-байтовый сегмент за payload, грузится на 0xE_F000. Fixture 1072 байта, SHA-256 `b1ba6078a4d9bc7a8d224980713d8242e4a708743ce4f56093b00a07c631ffaf`, пересчитана независимо через `python3 -I` без кода Rust (manifest hash, суммы всех таблиц и RSDP, ссылки RSDP -> XSDT -> таблицы, FADT -> DSDT и FACS). Валидатор `od-machine` принимает только `load_base == 0xE_0000`, `rsdp_address == load_base`, `facs_address == 0xE_F000`; проверяет RSDP (сигнатура, обе суммы, revision, RSDT = 0, XsdtAddress = XSDT blob), XSDT (записи ровно на FADT/MADT/MCFG blob), FADT (`X_DSDT`, `X_FIRMWARE_CTRL` и 32-битные формы), FACS, нулевые промежутки (в том числе между каталогом и payload); инверсия любого бита шапки, каталога и payload на fixture отвергается. Закрыт долг S4-31d/e (подробно в отчёте): комментарии границ, нулевой промежуток, жёсткий `load_base`, единый `to_bytes` в `firmware_board.rs`, `declared` в `validate_fadt`, doc `validate_facs`. Граница: не проверяются loader, E820, размещение в RAM (S4-37) и принятие таблиц Linux (S4-38). Отчёт: `docs/reports/STAGE_4_S4-31G_2026-11-08.md`, данные: `docs/reports/data/stage4/s4-31g-full-blob.json`.

## S4-24e UART IRQ4

57. S4-24e проверяет ограниченную цепочку COM1 RX → IRQ4 → IOAPIC INTIN4 (edge, `set_intin4`) → Local APIC → гостевой обработчик на плоском профиле `Stage4UartIrqProbeV1` (identity-память, без paging). Устройство: `cargo test -p od-machine --lib uart` (IER RDA/THRE; приоритет RDA над THRE; RDA снимается только чтением RBR; повторное чтение IIR без побочных эффектов; THRE снимается чтением IIR, только если он источник, или записью THR; OUT2 гейтит только линию, но не IIR; в IER принимаются биты 0–1, в MCR только OUT2, остальное `UnsupportedConfiguration`). CPU: `cargo test -p od-isa --lib lidt` (новый `step_flat_lidt` делит тело с `step_paged_lidt`; CR0/CR3/CR4 должны отсутствовать). Машина: `cargo test -p od-machine --test stage4_uart_irq_probe` (10 тестов; образ собирается в тесте побайтно, toolchain не нужен): позитив RX → IRR → ISR → чтение IIR (0x04) и RBR → OUT → EOI → IRETQ → HLT с порядком в trace; RX до записи IER (ребро ждёт при IF=0, вход после STI;HLT); два байта с EOI между; негативы: IER=0 (нет `uart_line`, IRR, входа в обработчик; байт остаётся в RBR), маска INTIN4 (ребро при маске теряется, снятие маски при всё ещё высокой линии ничего не доставляет: S4-26a/b не воспроизводят замаскированное ребро), второй IRQ без EOI (IRR=2, вход=1, ISR остаётся, вторая доставка не происходит), HLT при IF=0; injected regressions «без EOI» и «без чтения RBR» ломают предикат приёмки `accepts`; бюджет, STOP раньше бюджета, разбиение на куски с тем же trace/state, повтор после idle, отказы загрузчика (размер, GDT, лишний gate, тип и селектор gate, LIDT на входе, IDTR, посторонние байты). Host-side мутация (убран вызов `set_intin4`) провалила 6 из 10 тестов. Builder: `python3 -m unittest tools/build/test_build_stage4_uart_irq_probe.py` (5 тестов: две сборки идентичны и совпадают с manifest; варианты отличаются и помечены `accepted_profile=false`; валидатор отвергает подмену кода при совпадающих ELF и raw, лишний gate и посторонний байт, несовпадение варианта). Builder → runner: `cargo test -p od-runner --test stage4_uart_irq_cli` (6 тестов на реальном clang/lld ядре: позитив и порядок trace; два байта; `--budget`, `--cancel-after`; IER=0; поздняя маска; регрессии `--omit-eoi` и `--omit-rbr-read` дают `completion_mismatch` и код 5; воспроизводимость; отказы загрузчика и аргументов). Измерено на `--rx 0x41@1000`: 42 шага (`quanta`), 40 retired, `clock_ns=1000`, trace из 21 события; на двух байтах — 55 шагов и 51 retired. Не заявлено: FIFO, baud timing (передача завершается сразу после OUT), прерывания LSR и modem status, level route, JIT, совместимость с Linux 8250, production scheduler, IRQ из CPL3, paged-вход в IRQ; S4-24 остаётся открытым. Отчёт: [S4-24e](../reports/STAGE_4_S4-24E_2026-11-08.md).
