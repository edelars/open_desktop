# Методика проверки машины: текущий срез

S4-00a evidence capture 2026-11-05 сверяет binary hashes по
[S2-40d](../reports/S2_40D_ARTIFACT_DIGEST_RECHECK.md), фиксирует recovered
Ubuntu 7.0.0-38.38 source/config hashes, совпадающие Build IDs и равенство 38
общих allocated file-backed ELF sections. Это не аутентифицирует mirror download и не
связывает source/config с binary; полный список ограничений и команд находится
в [S4-00a report](../reports/STAGE_4_S4-00A_2026-11-05.md). S4-00/S4-01 и
полная source provenance остаются открытыми.

## Область

Эта методика покрывает S4-02—S4-10a и bounded S4-06b: валидируемую physical
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

## Интерпретация

Успешные локальные тесты подтверждают только перечисленный Rust-контракт,
flat identity integration, tracked RAM-only effects и targeted `od-isa`
PFEC/record tests, pure translation probe, S4-10a rejection boundary, S4-11a
cross-page admission, S4-12a snapshot bridge, S4-13a pure-cache experiment,
S4-14a cache lifecycle boundary, S4-21a diagnostic vCPU session и S4-22a
caller-driven clock/RDTSC boundary и S4-23a bounded deadline queue.
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
