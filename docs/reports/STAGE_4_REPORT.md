# Отчёт этапа 4

## Текущий статус

На 2026-11-04 принят только локальный memory/bus/bridge/translation slice
S4-02—S4-07. Этап 4 не завершён: S4-00—S4-01, integrated faults/delivery, vCPU/events, devices, JIT
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
- `MachineBus` добавляет отдельную checked PIO map, validated MMIO bindings и
  синхронный `BusDevice` dispatch без partial/cross-window операций.
- MMIO/PIO widths, PIO endpoint/overflow, device errors и отсутствие побочных
  эффектов до commit проверены отдельными unit tests; RAM callback не вызывается
  для device writes.
- `IdentityMemoryAdapter` подключает существующий `od_isa::execute::run` к
  production `MachineBus` в диагностическом flat identity mode. Regression
  выполняет fetch/load/store, затем unmapped load и проверяет CPU state/RIP,
  retirement count, RAM и единственный callback.
- `probe_long64_4k` выполняет pure uncached four-level/4 KiB translation probe
  с canonical48, physical36, cumulative permissions и literal PFEC. Он
  отклоняет ET=0/unsupported snapshots, сохраняет trace/table-read errors и не
  меняет A/D bits или CPU state.

## Проверки

Выполнены:

- `cargo fmt --all -- --check` — успешно;
- `cargo check -p od-machine --all-targets --offline` — успешно;
- `cargo test -p od-machine --offline -- --test-threads=1` — 6 тестов успешно
  для memory slice; после S4-05 — 10; после S4-06 — 16; после S4-07 — 23
  теста успешно;
- `git diff --check` — успешно;
- независимое ревью `gpt-6.1-sol/high` — для S4-02—S4-06 подтверждённые
  замечания исправлены с повторным approve; для S4-07 найден ET=0 P2, затем
  добавлена fail-closed проверка и повторное ревью: **approve**.

Архитектурный анализ `gpt-6-astra/high` подтвердил границу среза и отсутствие
необходимости добавлять зависимости на `od-isa`, `od-jit` или `od-host`.

## Границы доказательства

Это local Rust evidence. Не выполнены и не заявляются: CPU-integrated guest
linear translation, TLB, A/D updates, page faults, descriptor delivery,
UART/APIC, JIT invalidation,
privileged hardware execution, Linux boot и userspace. S4-05 проверяет только
bus/device boundary, S4-06 — только flat identity integration, а S4-07 — только
pure translation probe, а не реальные
гостевые `IN/OUT` instructions или page-fault delivery.

Следующий срез должен добавить отдельный bus contract, затем CPU memory bridge и
uncached Long64 page walker; ошибки этого слоя нельзя подменять host marker или
автоматически превращать в guest exception.
