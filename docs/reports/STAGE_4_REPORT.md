# Отчёт этапа 4

## Текущий статус

На 2026-11-05 приняты локальные memory/bus/bridge/translation,
bounded page-fault-record, tracked RAM access, S4-01a baseline, S4-10a
admission-boundary, bounded S4-06b CPU-connected paged MOV, bounded S4-06c
CPU-connected paged MOV to MMIO, bounded S4-11b cross-page qword read and
reviewed S4-11c cross-page dword read. Также приняты bounded S4-21b
paged probe quantum, bounded S4-23c UART RX event delivery, bounded S4-24a
COM1 polling TX, bounded S4-24b CPU PIO and bounded S4-24c paged CPU PIO
slices. Этап 4 не завершён: S4-00—S4-01,
integrated faults/delivery, vCPU/events, полный UART/IRQ, JIT
integration, hardware cases и Linux userspace остаются открытыми. Ограниченный
срез S4-32a принят после независимого ревью и зафиксирован ниже; родительский
S4-32 и этап 4 остаются открытыми.

## S4-32a: reproducible freestanding flat probe

The accepted bounded S4-32a slice adds a real assembly/linker fixture, a strict
`Stage4FlatProbeV1` raw-image loader, and a runner path that invokes the
existing bounded paged quantum. The image is exactly `0x9000` bytes at GPA
`0x1000`, with fixed page tables, code/data/stack mappings and a direct
Long64/CPL0 handoff. The guest executes `MOV RAX,[RBX]`, `OUT DX,AL`, `HLT`;
the runner reports the UART byte actually produced by the guest and never
injects an expected output.

Two clean builds with payload `0x1122334455667788` matched raw SHA-256
`e78a7af7306e31fa316f1d64332ea4bd64abf0e854883106b1933f68bef97204`.
Loader tests passed 4/4, runner CLI tests 5/5, and the build-to-CLI acceptance
test passed 1/1. Real CLI smoke covered both halted output and budget
exhaustion with partial state. The complete runner
all-targets suite was started but not completed, so no aggregate pass count is
claimed. Independent `gpt-6.1-sol/high` reviews found no remaining P0–P3
findings. The follow-up fixed the initial documentation, definedness,
build-to-CLI and provenance findings: the runner example now names
`flat_probe.bin` and renders as valid Markdown, stale toolchain-blocker wording
was removed, the handoff preserves `0x0cd7`, real artifacts are exercised by a
permanent test, and the full objcopy version is recorded.
Full artifact, loader, runner and review evidence is in the
[S4-32a report](STAGE_4_S4-32A_2026-11-05.md) and
[JSON evidence](data/stage4/s4-32a-flat-probe.json).

## S4-32b: freestanding store→load→UART probe

S4-32b adds a separate `Stage4StoreLoadProbeV1` artifact with real guest code
`MOV [RBX],RCX → MOV RAX,[RBX] → OUT DX,AL → HLT`. The image keeps the fixed
S4-32a paging/layout contract but stores distinct initial and store-value
qwords. The loader hands the artifact's store-value to `RCX`; the guest, not
the host runner, performs the RAM write and serial output.

The machine test and permanent build-to-CLI test verify four retirements,
full data/RAX/UART/HLT state, same-input raw reproducibility, two different
store-values, budget 1/2/3 committed prefixes, corrupted-code rejection and
equal-data rejection. This bounded slice does not claim general kernel loading,
stack execution, exception repair, cross-page/alias behavior, IRQ/APIC,
HLT wakeup, JIT, Linux boot or S4-33 completion. Evidence:
[report](STAGE_4_S4-32B_2026-11-05.md) and
[JSON](data/stage4/s4-32b-store-load-probe.json).

S4-23c adds a bounded `UartRxEventPump` between `EventQueue` and one UART RX
endpoint. Its dispatch order is generation → STOP → finite event budget →
readiness. Payload capacity is fixed; the queue owns generations, absolute
deadlines and ordering. Successful injection consumes exactly one event and
payload. `ReceiverBusy` preserves the earliest event, payload, deadline and
order for retry. Reset/stale-generation invalidation follows `EventQueue`; the
pump does not advance `VirtualClock`, change CPU state or wake HLT. Evidence:
[S4-23c report](STAGE_4_S4-23C_2026-11-05.md) and
[JSON evidence](data/stage4/s4-23c-uart-rx-events.json).

The complete `od-machine` suite passed **129 tests (81 unit + 48 integration)**,
including `uart_rx_events` **7/7**. Package/workspace checks, formatting, layout
and diff checks passed; independent review approved with no P0–P3 findings.
S4-23, S4-24 and Stage 4 remain open. This slice does not claim IRQ/APIC, baud
timing, HLT wakeup, host wait or Linux boot. The S4-32a evidence below records
the bounded freestanding toolchain path; it is not a general kernel or Linux
loader.

S4-21b принимает `PagedCpl0ProbeV1`: `step_paged_probe` допускает только
`MOV RAX,[RBX]`, `MOV [RBX],RCX`, bare `IN AL,DX`, `OUT DX,AL` и `HLT`.
CPU проверяет admission и CR0/CR4; `run_paged_probe` валидирует CR3/EFER и
исполняет finite quantum через page-aware memory. На каждой границе один
decode; порядок выхода — STOP, budget exhausted, затем fetch. HLT устанавливает
guest halted state, а повторный запуск halted CPU ничего не исполняет.
Ранее committed effects и tracked A/D callbacks сохраняются при позднем
fault/exit, а точный fault record остаётся доступен вызывающему коду.
Проверки и ограничения — в [отчёте S4-21b](STAGE_4_S4-21B_2026-11-05.md)
и [JSON evidence](data/stage4/s4-21b-paged-probe.json).
`od-isa`: 2,014 unit + 25 integration; `od-machine`: 116; workspace check,
fmt, layout и diff прошли. `graphify update .` обновил code graph; docs-aware
semantic extraction не завершена. Это не доказывает exception
delivery/CR2 update, kernel/Linux boot или IRQ/APIC; S4-21, S4-24, S4-32 и
Stage 4 остаются открытыми.

В S4-24c принят `PagedCpl0PioU8V1` через CPU `step_paged_pio_u8`: bare `EC`/`EE`
в Long64 CPL0, с already-selected `GuestMemory` и CPU-side CR0/CR4/admission
validation. Machine `step_paged_pio_u8` отдельно валидирует CR3/EFER через
существующую `Long64ProbeConfig` для four-level 4 KiB paging и передаёт реальный
page-aware memory adapter. Fetch следует таблицам страниц с
`AccessOrigin::Explicit`.
Проверены not-present PFEC `0x10` и NX protection PFEC `0x11`. Сквозной machine
тест выполняет `MOV RAX,[RBX]` → `OUT DX,AL` без host mutation и передаёт
definedness между шагами; UART output совпадает с байтом из guest RAM. При PIO
failure уже совершённые A/D записи и callbacks сохраняются.

Full tests: `od-isa --all-targets` — 2,014 unit, 6 paged и 7 legacy тестов;
`od-machine --all-targets` — 105 тестов, включая 7 paged. Workspace check,
formatting, layout, diff, JSON/link checks прошли; независимое
`gpt-6.1-sol/high` review одобрило code diff без замечаний P0–P3. См. [отчёт S4-24c](STAGE_4_S4-24C_2026-11-05.md)
и [JSON evidence](data/stage4/s4-24c-paged-pio.json). Не заявляются mixed
runner, exception delivery/CR2, IOPL/TSS, IRQ/APIC, scheduler, baud timing, JIT
или Linux support. Parent S4-24 и Stage 4 остаются открытыми.

В S4-24b реализован `DiagnosticCpl0PioU8V1`: один bare `EC`/`EE` за вызов,
Long64 CPL0, CR0/CR4 `None`, instruction fetch через flat identity RAM.
Успешная инструкция делает один byte PIO callback и одну retirement;
неуспешные fetch/PIO не меняют CPU state. `IN AL,DX` сохраняет старшие 56
бит RAX. UART использует offset `0` как RBR при чтении и THR при записи;
`LSR.DR` — bit `0`, чтение pending RBR очищает его, а пустой RBR возвращает
`DeviceError::WriteOnly`. Он сохраняет ранее добавленный bounded TX path.
Проверки: `od-isa`
2014 unit + 7 integration tests, `od-machine` 98 tests, package/workspace
checks, fmt и diff check прошли. Независимое `gpt-6.1-sol/high` ревью полного
code-среза одобрило его без замечаний P0–P3. См. [отчёт S4-24b](STAGE_4_S4-24B_2026-11-05.md)
и [JSON evidence](data/stage4/s4-24b-pio.json). Не заявляются IOPL/TSS,
другие widths, immediate/REP/string I/O, IRQ/APIC или Linux support; parent
S4-24 remains open.

В S4-24a (baseline `6aeb2dd`) добавлен bounded UART polling path для COM1
`0x3F8–0x3FF` через существующую PIO bus с ограниченным TX состоянием.
`Uart16550PollingTx` и
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

В S4-06c на baseline `833ea99` две поддержанные S4-06b MOV-формы получили
доступ к qword MMIO через существующий `MachineBus` binding после translation
и permission checks. Тесты подтверждают полный `DeviceAccess`/payload, один
dispatch, порядок A/D callbacks перед устройством, отсутствие вызова на
permission fault и сохранение CPU state/нулевого retirement при backend error.
Финальное независимое `gpt-6.1-sol/high` review одобрило срез без P0—P3;
11 focused `paged_mov_step` тестов, package check, форматирование и diff check
прошли. См. [контракт S4-06c](../specs/MACHINE_CONTRACT.md#s4-06c-cpu-connected-paged-mov-to-mmio),
[отчёт](STAGE_4_S4-06C_2026-11-05.md) и
[evidence manifest](data/stage4/s4-06c-paged-mmio.json). Родительский S4-06
остаётся открытым.

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
  общий split path не добавлен. S4-11b ниже добавляет только один CPU-connected
  qword RAM load через две страницы.
- S4-11b поддерживает только `MOV RAX,[RBX]` (`48 8B 03`) для qword read через
  две 4 KiB RAM pages: обе translation/permission/region проверки предшествуют
  operand read; A effects остаются при последующем fault; D не ставится; bytes
  читаются во временный buffer, CPU destination и caller buffer обновляются
  только при полном успехе. Все семь boundary offsets и literal nonidentity,
  noncontiguous mappings проверены. См. [контракт](../specs/MACHINE_CONTRACT.md#bounded-cross-page-qword-read-s4-11b),
  [отчёт](STAGE_4_S4-11B_2026-11-05.md) и
  [manifest](data/stage4/s4-11b-cross-page-read.json). Родительский S4-11 открыт.
- S4-11c добавляет отдельный `PagedMovCrossPageDwordReadLong64V1`, допускающий
  только bare `MOV EAX,[RBX]` (`8B 03`). Same-page RAM control и crossing offsets
  `0xffd..0xfff` проверены на двух non-contiguous RAM pages. Оба walk/permission/
  RAM admission предшествуют read; результат zero-extends в EAX, RIP advances на
  2, retirement происходит один раз только после полного чтения; second-page
  #PF сохраняет full RAX/RIP, reads не ставят D, alias sampling следует
  S4-11b walk-then-sample. Cross-page MMIO не dispatch-ится. Проверки и границы:
  [S4-11c report](STAGE_4_S4-11C_2026-11-05.md), [machine contract](../specs/MACHINE_CONTRACT.md#bounded-cross-page-dword-read-s4-11c),
  [manifest](data/stage4/s4-11c-cross-page-dword-read.json). Независимое
  `gpt-6.1-sol/high` review принято после исправления одного P3 evidence
  несоответствия; P0—P2 замечаний нет. Родительский S4-11 остаётся открытым.
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
- S4-23b добавляет `EventGeneration` с начальным значением `0`, stale-generation
  checks до любых поисков и изменений очереди, reset с checked increment и
  проверку поколения извлечённых событий. Успешный reset очищает entries и
  sequence, сохраняя capacity и `VirtualClock`; stale reset и
  `GenerationExhausted` атомарны. Порядок и абсолютная семантика deadlines S4-23a
  сохранены. Это не добавляет dispatch, periodic timers, IRQ, HLT wakeup,
  concurrent ownership или Linux boot. Подробности и evidence находятся в
  [отчёте S4-23b](STAGE_4_S4-23B_2026-11-05.md).
- S4-23c добавляет bounded `UartRxEventPump` для доставки RX bytes в один UART.
  Порядок dispatch — generation → STOP → budget → readiness; capacity payload
  bounded, а поколения, deadlines и порядок принадлежат `EventQueue`. Успешная
  доставка инжектирует byte ровно один раз и удаляет событие/payload; при
  `ReceiverBusy` самое раннее событие и его deadline/order сохраняются для
  повтора. Reset/stale invalidation следует контракту очереди; clock, CPU и HLT
  не изменяются. Подробности в [отчёте S4-23c](STAGE_4_S4-23C_2026-11-05.md)
  и [JSON evidence](data/stage4/s4-23c-uart-rx-events.json).

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
separate-call evidence. S4-11b/c — только bounded qword/dword read profiles; они не закрывают
cross-page stores, CPU restart, REP iteration, stack recovery или RMW atomicity.
S4-06b не доказывает полную paging matrix, TLB/invalidation, CR2
commit, exception delivery/retry, general cross-page operands, REP/stack/atomic
semantics, JIT/IRQ integration, SMP или Linux boot. S4-06c подтверждает только
qword MMIO через два MOV forms и не подтверждает full device integration.
ISSUE-054 остаётся открытой для
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
S4-23b доказывает только bounded reset/generation contract и stale-event
validation; он не доказывает dispatch, IRQ, periodic timers, HLT wakeup,
concurrent access или Linux boot. S4-23 и Stage 4 остаются открытыми.
S4-23c доказывает только bounded RX event delivery to UART; он не подтверждает
IRQ/APIC, baud timing, HLT wakeup, host wait или Linux boot. S4-23, S4-24 и Stage
4 остаются открытыми. S4-32a отдельно подтверждает bounded freestanding
toolchain/flat-probe path, но не general kernel loading или Linux boot.

Открыты manifest S4-00, реализация 2 MiB/1 GiB и необходимых режимов, injected
table-write failure, полная rights/error matrix, двухстраничная transaction
semantics, guest MOV CR3/WRMSR transitions, CPU execution integration, startup
state, production TLB choice и system acceptance. S4-13a измеряет только
synthetic pure-probe workload; ошибки этого слоя нельзя подменять host marker
или автоматически превращать в guest exception.

## S4-32c: freestanding stack→UART probe

The S4-32c slice adds a real `Stage4StackProbeV1` artifact with exact
entry bytes `51 58 EE F4`. The fixed loader passes the artifact payload qword
to `RCX`; the opt-in stack paged quantum performs `PUSH RCX`, `POP RAX`, `OUT DX,AL`
and `HLT` against the mapped guest stack and COM1 UART. Evidence checks the
stack qword at GPA `0xaff8`, restored `RSP`, `RAX`, `RIP`, retirement, UART,
strict image rejection, A/D callback ordering and fault metadata through
machine and build-to-runner tests. Independent `gpt-6.1-sol/high` review
approved this bounded slice with no P0–P3 findings.
The boundary is local Long64/CPL0
interpreter evidence only; exception delivery/repair, CR2, cross-page
transactions, IRQ/APIC, HLT wakeup, JIT, Linux, hardware and parent S4-32/S4-33
remain open. See the [slice report](STAGE_4_S4-32C_2026-11-05.md) and
[evidence JSON](data/stage4/s4-32c-stack-probe.json).

## S4-33a: guest RAM alias store→load

The separate `Stage4AliasProbeV1` artifact maps data GVA `0x800000` and stack
GVA `0xa00000` to GPA `0x9000`. Its distinct initial/store qwords let the guest
change RAM through the first mapping and load the changed value through the
stack alias. Acceptance verifies RAM, RAX, callback order, manifest layout and
UART; a guest HLT by itself is insufficient, including in the injected broken
alias case. See the [S4-33a slice report](STAGE_4_S4-33A_2026-11-05.md) and
[evidence](data/stage4/s4-33a-ram-alias.json). This is uncached interpreter
evidence only; independent `gpt-6.1-sol/high` review approved the bounded case
with no P0–P2 findings. S4-33 and Stage 4 remain open.
