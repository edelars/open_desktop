# Отчёт этапа 4

## Текущий статус

На 2026-11-05 принят только локальный memory/bus/bridge/translation,
bounded page-fault-record, tracked RAM access и S4-10a admission-boundary slice
S4-02—S4-23a. Этап 4 не завершён: S4-00—S4-01,
integrated faults/delivery, vCPU/events, devices, JIT
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
- `od-isa` задаёт общий literal-tested PFEC helper и `PageFaultRecord` с vector
  14, обязательными error code/linear address, saved RIP, access kind и полным
  context. `od-machine::probe_instruction_access` преобразует только probe
  `PageFault`; translation и остальные ошибки сохраняют отдельный тип. Запись
  не меняет CR2, `LongModeState`, RAM или A/D bits.
- `access_long64_4k_ram` добавляет ограниченный tracked RAM-only путь для Fetch,
  Read и Write: Read/Write принимают ширины 1/2/4/8, Fetch остаётся byte-only;
  отсутствующие A bits коммитятся по мере обхода, D — только после полной
  проверки прав и RAM operand, затем коммитится operand.
  Уже совершённые A/D effects не откатываются при последующем fault; MMIO,
  cross-page и huge-page paths намеренно отвергаются.
- S4-10a закрепляет admission boundary для неподдержанных huge-page entries:
  present PS на PDPT/PD возвращает точный `UnsupportedPageSize`, не спускается
  ниже и не трогает operand; уже committed A у предков сохраняется. PML4 bit 7
  остаётся reserved-bit fault, PT bit 7 не меняет 4 KiB admission. 2 MiB/1 GiB
  leaves и новые paging modes не реализованы до manifest S4-00.
- S4-11a закрепляет cross-page admission boundary: Read/Write, пересекающие
  4 KiB, возвращают `CrossesPage` до чтения таблиц, без trace, A/D effects,
  callback или operand commit. Отдельные same-page calls коммитятся независимо;
  двухстраничный transaction/split path не добавлен.
- S4-12a добавляет `Long64ProbeConfig::from_cpu_state`: CR0 и CR4 читаются из
  `LongModeState` с явной ошибкой отсутствующего регистра, CR3 и EFER остаются
  явными входами, а вся существующая validation переиспользуется без defaults,
  masking или mode-transition effects. Snapshot immutable и диагностический.
- S4-13a добавляет opt-in one-entry pure-translation cache и release comparison
  harness. Три процесса завершили 600 retained paired samples на synthetic
  profiles; результаты включают locality wins и two-page alternating slowdown,
  поэтому production TLB capacity/policy не выбрана.
- S4-14a закрепляет host-controlled lifecycle границу: `clear()` идемпотентен,
  не пишет RAM/A-D, failed replacement не resident, а remap и изменения RW/NX
  наблюдаются только после завершения старого immutable-borrow scope и создания
  нового cache. Guest invalidation и CPU hooks не добавлены.
- S4-21a добавляет синхронную bounded diagnostic vCPU session над одним
  `LongModeState` и `MachineBus`: runnable/paused/stopped/terminal states,
  finite instruction budget, сохранение `rflags_defined`, STOP callback и
  отдельные fault/unsupported/backend exits. Полный vCPU lifecycle не закрыт.
- S4-22a добавляет caller-driven `VirtualClock` и bounded RDTSC boundary:
  checked `u128` conversion, atomic overflow rejection, explicit time advance,
  сохранение `rflags_defined`, TSD/CPL/fetch checks и отсутствие auto-advance.
  RDTSCP, MSR/CPUID, HLT progression, IRQ/deadlines и host-clock coupling не
  реализованы.
- S4-23a добавляет bounded fixed-capacity очередь одноразовых absolute
  deadlines с deterministic `(deadline, sequence)` ordering, explicit cancel/
  reschedule и one-at-a-time `pop_due`. Очередь не двигает clock и не dispatch-ит
  CPU/device/IRQ events.

## Проверки

Выполнены:

- `cargo fmt --all -- --check` — успешно;
- `cargo check -p od-machine --all-targets --offline` — успешно;
- `cargo test -p od-machine --locked --offline -- --test-threads=1` — 6 тестов успешно
  для memory slice; после S4-05 — 10; после S4-06 — 16; после S4-07 — 23
  теста успешно; после S4-08 — 25; после S4-09 — 33; после S4-10a — 40;
  после S4-11a — 46; после S4-12a — 49; после S4-13a — 55 feature-тестов
  `od-machine` и 3 benchmark-теста успешно;
- `cargo test -p od-isa --locked --offline page_fault` — 3 targeted PFEC/record
  теста успешно; полный `cargo test -p od-isa --locked --offline` — 2006
  тестов успешно;
- `git diff --check` — успешно;
- `cargo test -p od-machine --features experimental-translation-probe-cache
  --locked --offline translation_probe_cache::tests` — 11 cache lifecycle
  тестов успешно; полный feature-enabled `od-machine` — 60 тестов успешно;
- `cargo check -p od-machine --all-targets --features
  experimental-translation-probe-cache --locked --offline` — успешно;
- `cargo test -p od-machine --locked --offline --test diagnostic_vcpu_api` — 7
  integration tests S4-21a/S4-22a успешно; targeted `od-isa` RDTSC и
  definedness tests успешно;
- независимое ревью `gpt-6.1-sol/high` — для S4-02—S4-06 подтверждённые
  замечания исправлены с повторным approve; для S4-07 найден ET=0 P2, затем
  добавлена fail-closed проверка и повторное ревью: **approve**; S4-08 получил
  `approve with nits`, после чего методика дополнена targeted `od-isa` command;
  S4-09 после исправления P2/P3 получил повторное **approve**; S4-10a после
  исправления тестовых P2 получил повторное **approve**; S4-11a после
  исправления fixture/assertion P2 получил повторное **approve**; S4-12a получил
  независимое **approve**; S4-13a получил независимое **approve** после
  исправления benchmark parity P2; S4-14a получил независимое **approve**,
  P0—P2 не обнаружены.
  S4-22a получил независимое **approve**; два P3 теста усилены и закрыты
  повторным ревью, новых P0—P2 нет.

Архитектурный анализ `gpt-6-astra/high` подтвердил границу среза и отсутствие
необходимости добавлять зависимости на `od-isa`, `od-jit` или `od-host`.

## Границы доказательства

Это local Rust evidence. Не выполнены и не заявляются: CPU-integrated guest
linear translation, TLB, полная matrix A/D semantics, exception delivery/CR2 commit, descriptor delivery,
UART/APIC, JIT invalidation,
privileged hardware execution, Linux boot и userspace. S4-05 проверяет только
bus/device boundary, S4-06 — только flat identity integration, S4-07—S4-08 —
pure translation probe с record construction, а S4-09 — tracked RAM-only probe,
S4-10a — только rejection boundary для unsupported huge pages, а не реальные
гостевые `IN/OUT` instructions, CPU page-fault delivery или
полный executor integration. S4-11a — только same-page/cross-page admission и
separate-call evidence, не CPU restart, REP iteration, stack recovery или RMW
atomicity. Independent review покрывает только bounded slices; полная
S4-08/S4-09/S4-10/S4-11 приёмка остаётся шире. S4-13a не доказывает
production TLB correctness, invalidation или guest workload benefit.
S4-14a не доказывает guest INVLPG/CR3/EFER semantics, generations,
global/PCID, CPU/JIT invalidation или concurrent memory ownership.
S4-21a не доказывает HLT wakeup, reset, IRQ delivery, MMU/JIT integration,
REP continuation или VMHost/IPC scheduling.
S4-22a не доказывает Linux-visible frequency policy, host monotonic time,
RDTSCP/MSR/CPUID, HLT progression или event deadlines.
S4-23a не доказывает dispatch, periodic timers, reset/generation, HLT wakeup,
IRQ delivery, host wait или cross-machine event ownership.

Открыты manifest S4-00, реализация 2 MiB/1 GiB и необходимых режимов, injected
table-write failure, полная rights/error matrix, двухстраничная transaction
semantics, guest MOV CR3/WRMSR transitions, CPU execution integration, startup
state, production TLB choice и system acceptance. S4-13a измеряет только
synthetic pure-probe workload; ошибки этого слоя нельзя подменять host marker
или автоматически превращать в guest exception.
