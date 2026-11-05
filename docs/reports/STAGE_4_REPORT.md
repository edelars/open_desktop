# Отчёт этапа 4

## Текущий статус

На 2026-11-05 приняты локальные memory/bus/bridge/translation,
bounded page-fault-record, tracked RAM access, S4-01a baseline, S4-10a
admission-boundary, bounded S4-06b CPU-connected paged MOV и bounded S4-24a
COM1 polling TX slices. Этап 4 не завершён: S4-00—S4-01,
integrated faults/delivery, vCPU/events, полный UART/IRQ, JIT
integration, hardware cases и Linux userspace остаются открытыми.

В S4-24a (baseline `6aeb2dd`) добавлен bounded TX-only UART для COM1
`0x3F8–0x3FF` через существующую PIO bus. `Uart16550PollingTx` и
`Uart16550Handle` разделяют одно состояние; передача продвигается только
явными host-вызовами. Проверены byte-only PIO, DLAB/DLL/DLM, 8N1 с ненулевым
divisor, THRE/TEMT transitions, holding/shift, busy/unsupported atomicity,
raw-byte completion и reset. Источник ограниченной register semantics —
[TI PC16550D datasheet, SNLS378C](https://media.digikey.com/pdf/Data%20Sheets/Texas%20Instruments%20PDFs/PC16550D.pdf).
Не заявляются полная 16550A совместимость, baud timing, RX/FIFO, IRQ4, гостевые
`IN/OUT`, Linux serial compatibility, boot или scheduler. Результаты: 7 UART
unit tests, 2 UART integration tests, 4 legacy `MachineBus` tests; package
all-targets check, fmt, layout и diff checks прошли. Независимое
`gpt-6.1-sol/high` ревью — approve без замечаний P0–P3. См. [отчёт S4-24a](STAGE_4_S4-24A_2026-11-05.md)
и [JSON evidence](data/stage4/s4-24a-uart.json). Родительский S4-24 открыт,
поскольку гостевой I/O path и IRQ не реализованы.

В S4-00a (2026-11-05) восстановлены и захэшированы exact-version source
package/tree inputs, config и три source-файла Ubuntu 7.0.0-38.38. Повторно
зафиксированы четыре binary hashes из S2-40d; Build IDs `vmlinux` и debug ELF
совпали, а read-only ELF64 section parser подтвердил равенство 38 общих
allocated file-backed sections (42 release sections, 57 debug sections).
Подписанные source/index inputs и `apt-provenance.json` отсутствуют, поэтому
authenticated source/config-to-binary mapping не подтвержден. Подробности и
локальные evidence paths — в
[отчёте S4-00a](STAGE_4_S4-00A_2026-11-05.md) и
[JSON manifest](data/stage4/s4-00a-source-audit-2026-11-05.json). Родительские
S4-00 и S4-01 остаются открытыми.

В S4-01a (2026-11-05) зафиксирован свежий исполняемый baseline диагностического
пути `od_isa::execute::run → IdentityMemoryAdapter → MachineBus → PhysicalMemory`:
успешные load/store и backend fault на hole `0x3000` без retirement или мутаций,
а также отдельный immutable page-walk snapshot control с разными CR3. Оба exact-
теста прошли по одному тесту. Это flat identity fixture, не production paging и
не Linux manifest; `#PF` delivery/retry, JIT, privileged hardware и boot остаются
вне среза. Evidence: [отчёт S4-01a](STAGE_4_S4-01A_2026-11-05.md) и
[JSON baseline](data/stage4/s4-01a-baseline-2026-11-05.json).

В S4-06b (baseline `b2a093c`) добавлен bounded путь CPU → tracked paged RAM
для двух точных Long64 MOV: `48 8B 03` и `48 89 0B`. CPU entry выполняет один
шаг с validated CR0/CR4, явным `Explicit` origin, сохранением исходных
mode/privilege/CPL и `rflags_defined`. Machine adapter использует
`Long64ProbeConfig::from_cpu_state` (canonical48, physical36, четыре уровня,
4 KiB, RAM-only), literal non-identity test mappings, существующий walker и
единственный RAM owner. A/D writes и callbacks сохраняются в фактическом
порядке при последующем отказе; failed read не меняет result buffer, а failed
operand write не коммитит operand частично. Alias page-table/operand допускается,
поэтому полной rollback RAM гарантии нет. См. [контракт CPU](../specs/CPU_CONTRACT.md#bounded-cpu-to-paged-ram-profile-pagedmovlong64v1-s4-06b),
[контракт машины](../specs/MACHINE_CONTRACT.md#s4-06b-cpu-connected-paged-mov),
[отчёт S4-06b](STAGE_4_S4-06B_2026-11-05.md) и
[JSON evidence](data/stage4/s4-06b-paged-mov.json). Родительский S4-06 остаётся
открытым.

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
- S4-01a повторно подтвердил существующий диагностический CPU-to-RAM путь через
  flat identity adapter и отдельно зафиксировал immutable page-walk snapshot;
  production CPU-integrated paging и Linux boot этим baseline не подтверждены.
- S4-06b подключил только две MOV формы к tracked RAM path и согласовал их
  `GuestMemory` guarantees с сохраняющимися A/D effects. Это не полная CPU
  paging integration и не rollback guarantee для других consumers.
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
  повторным ревью, новых P0—P2 нет; S4-01a получил независимое **approve**,
  P1—P3 не обнаружены; S4-06b получил независимое **approve**, ревьюер
  повторно выполнил 2,014 `od-isa` unit tests и 85 `od-machine` tests, P0—P3
  замечаний нет.

Архитектурный анализ `gpt-6-astra/high` подтвердил границу среза и отсутствие
необходимости добавлять зависимости на `od-isa`, `od-jit` или `od-host`.

## Границы доказательства

Это local Rust evidence. Не выполнены и не заявляются: CPU-integrated guest
linear translation, TLB, полная matrix A/D semantics, exception delivery/CR2 commit, descriptor delivery,
UART/APIC, JIT invalidation,
privileged hardware execution, Linux boot и userspace. S4-05 проверяет только
bus/device boundary; S4-06 flat identity integration и S4-06b two-form paged
MOV slice не закрывают родительскую задачу S4-06. S4-07—S4-08 —
pure translation probe с record construction, а S4-09 — tracked RAM-only probe,
S4-10a — только rejection boundary для unsupported huge pages, а не реальные
гостевые `IN/OUT` instructions, CPU page-fault delivery или
полный executor integration. S4-11a — только same-page/cross-page admission и
separate-call evidence, не CPU restart, REP iteration, stack recovery или RMW
atomicity. S4-06b не доказывает полную paging matrix, TLB/invalidation, CR2
commit, exception delivery/retry, cross-page operands, REP/stack/atomic
semantics, JIT/IRQ/devices, SMP или Linux boot. ISSUE-054 остаётся открытой для
cross-page/partial progress, REP/RMW и иных rollback consumers. Independent
review покрывает только bounded slices; полная
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
