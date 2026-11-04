# Отчёт этапа 4

## Текущий статус

На 2026-11-04 принят только локальный memory slice S4-02—S4-04. Этап 4 не
завершён: S4-00—S4-01, page walker, faults/delivery, vCPU/events, devices, JIT
integration, hardware cases и Linux userspace остаются открытыми.

## Реализовано

- `od-machine` получил `GuestPhysicalAddress`, `GuestPhysicalRange`,
  `MachineMemoryMap` и типы region classification.
- Карта сортируется, проверяет physical width 1—64, checked bounds и overlap.
- `PhysicalMemory` владеет zeroed RAM backing и проверяет суммарный
  `RamLimits` до allocation.
- Checked byte/scalar reads and writes используют little-endian encoding.
- Cross-region, hole, ROM, reserved и overflow accesses отклоняются без
  частичного изменения; committed-write callback не вызывается при отказе и
  вызывается один раз после успешной записи.
- Width 64 отдельно проверен для диапазона, содержащего `u64::MAX`.

## Проверки

Выполнены:

- `cargo fmt --all -- --check` — успешно;
- `cargo check -p od-machine --all-targets --offline` — успешно;
- `cargo test -p od-machine --offline -- --test-threads=1` — 6 тестов успешно;
- `git diff --check` — успешно;
- независимое ревью `gpt-6.1-sol/high` — сначала найдено 2 P2, затем после
  исправлений повторное ревью: **approve**, оставшихся P0/P1/P2 нет.

Архитектурный анализ `gpt-6-astra/high` подтвердил границу среза и отсутствие
необходимости добавлять зависимости на `od-isa`, `od-jit` или `od-host`.

## Границы доказательства

Это local Rust evidence. Не выполнены и не заявляются: guest linear translation,
TLB, A/D bits, page faults, descriptor delivery, MMIO/PIO, UART/APIC, JIT
invalidation, privileged hardware execution, Linux boot и userspace.

Следующий срез должен добавить отдельный bus contract, затем CPU memory bridge и
uncached Long64 page walker; ошибки этого слоя нельзя подменять host marker или
автоматически превращать в guest exception.
