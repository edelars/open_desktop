# Методика проверки машины: текущий срез

S4-00a evidence capture 2026-11-05 сверяет binary hashes по
[S2-40d](../reports/S2_40D_ARTIFACT_DIGEST_RECHECK.md), фиксирует recovered
Ubuntu 7.0.0-38.38 source/config hashes, совпадающие Build IDs и равенство 38
общих allocated file-backed ELF sections. Это не аутентифицирует mirror download и не
связывает source/config с binary; полный список ограничений и команд находится
в [S4-00a report](../reports/STAGE_4_S4-00A_2026-11-05.md). S4-00/S4-01 и
полная source provenance остаются открытыми.

## Область

Эта методика покрывает S4-02—S4-10a: валидируемую physical map, production RAM
owner, committed-write notification, синхронную MMIO/PIO bus, диагностический
CPU memory bridge, uncached translation probe и bounded tracked RAM access. Она не проверяет CPU-integrated
paging, guest exception delivery, APIC, JIT, firmware или Linux.

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
