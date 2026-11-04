# Методика проверки машины: текущий срез

## Область

Эта методика покрывает S4-02—S4-07: валидируемую physical map, production RAM
owner, committed-write notification, синхронную MMIO/PIO bus, диагностический
CPU memory bridge и uncached translation probe. Она не проверяет CPU-integrated
paging, guest exception delivery, APIC, JIT, firmware или Linux.

## Локальный набор

Команды выполняются из корня checkout:

```text
cargo fmt --all -- --check
cargo check -p od-machine --all-targets --offline
cargo test -p od-machine --offline -- --test-threads=1
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

## Интерпретация

Успешные локальные тесты подтверждают только перечисленный Rust-контракт и
flat identity integration.
и pure translation probe. Они не являются physical x86 evidence и не
подтверждают, что guest 4 KiB
страницы уже имеют независимые paging permissions. Отдельные failure values не
являются автоматически архитектурными `#PF`.

После изменения кода Graphify обновляется командой `graphify update .`. При
переходе к MMU/CPU-срезам каждый тест должен дополнительно сохранять fault state,
CR2, A/D effects и source of expected result.
