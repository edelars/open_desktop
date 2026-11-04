# Методика проверки машины: текущий срез

## Область

Эта методика покрывает S4-02—S4-05: валидируемую physical map, production RAM
owner, committed-write notification и синхронную MMIO/PIO bus. Она не проверяет
paging, guest exceptions, APIC, JIT, firmware или Linux.

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

## Интерпретация

Успешные локальные тесты подтверждают только перечисленный Rust-контракт.
Они не являются physical x86 evidence и не подтверждают, что guest 4 KiB
страницы уже имеют независимые paging permissions. Отдельные failure values не
являются автоматически архитектурными `#PF`.

После изменения кода Graphify обновляется командой `graphify update .`. При
переходе к MMU/CPU-срезам каждый тест должен дополнительно сохранять fault state,
CR2, A/D effects и source of expected result.
