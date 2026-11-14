# Отчёт этапа 4

## Текущий статус

На 2026-11-05 приняты локальные memory/bus/bridge/translation,
bounded page-fault-record, tracked RAM access, S4-01a baseline, S4-10a
admission-boundary, bounded S4-06b CPU-connected paged MOV, bounded S4-06c
CPU-connected paged MOV to MMIO, bounded S4-11b cross-page qword read and
reviewed S4-11c cross-page dword read. Также приняты bounded S4-21b
paged probe quantum, bounded S4-23c UART RX event delivery, bounded S4-24a
COM1 polling TX, bounded S4-24b CPU PIO, bounded S4-24c paged CPU PIO and
bounded S4-24d guest UART RX→IN→OUT echo slices. S4-11e cross-page word read is implemented and accepted after independent
review; its findings were fixed and no P0—P3 items remain. S4-11f word-MOV
cross-page fetch is implemented and accepted after independent review with no
remaining P0—P3 findings. Этап 4 не завершён: S4-00—S4-01,
integrated faults/delivery, vCPU/events, полный UART/IRQ, JIT
integration, hardware cases и Linux userspace остаются открытыми. Ограниченный
срез S4-32a принят после независимого ревью и зафиксирован ниже; родительский
S4-32 и этап 4 остаются открытыми.

Bounded S4-20c now completes a paged CPL3 `SYSCALL` → CPL0 kernel store →
`SYSRETQ` → CPL3 load roundtrip and is accepted after independent review with
no remaining P0–P3 findings. Its exact scope, checks and limitations are in the
[S4-20c report](STAGE_4_S4-20C_2026-11-06.md) and
[JSON evidence](data/stage4/s4-20c-paged-syscall-roundtrip-2026-11-06.json).
Parent S4-20 and Stage 4 remain open.

S4-20e implements a bounded CPL3 `MOV CR0,RAX` privilege-fault entry after
IRETQ. The exact instruction produces `#GP(0)`, the exact LOCK form produces
`#UD`, and tracked fetch faults retain precedence and paging effects. Direct
system coverage now includes non-present fetch PFEC `0x14` for the new entry.
The implementation checks and its remaining review status are recorded in the
[S4-20e report](STAGE_4_S4-20E_2026-11-07.md) and
[JSON evidence](data/stage4/s4-20e-paged-cpl3-mov-cr0-2026-11-07.json).
The follow-up Sol finding was corrected; independent code and documentation
reviews passed with no remaining findings. S4-20e is accepted; S4-20 and Stage
4 remain open.

S4-20d adds a bounded CPL3 paged store→load path after the accepted IRETQ and
load, with tracked user/supervisor write protection and exact fault evidence.
It is accepted after independent review with no P0–P3 findings. See the
[S4-20d report](STAGE_4_S4-20D_2026-11-06.md) and
[JSON evidence](data/stage4/s4-20d-paged-cpl3-store-load-2026-11-06.json).
Parent S4-20 and Stage 4 remain open.

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

В S4-24d добавлен строгий freestanding `Stage4UartEchoProbeV1` (`IN AL,DX` →
`OUT DX,AL` → `HLT`) и отдельный runner path. Внешний RX byte проходит через
session RX generation/deadline queue и UART; success отдельно сверяет expected
byte, guest RAX, RIP/retirement и фактически переданный UART TX. Пустой RBR
остаётся PIO error. Runner CLI — 9/9, builder→CLI — 2/2, 13 paged-session tests,
runner all-targets check, fmt и diff прошли. Layout check остаётся на известной
ISSUE-056 (`smoke/`). Независимые code/docs reviews чистые. Не заявляются guest
LSR polling loop, baud timing, IRQ/APIC, HLT wakeup или полное S4-24. Подробности:
[отчёт](STAGE_4_S4-24D_2026-11-07.md), [manifest](data/stage4/s4-24d-uart-echo.json).

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
- S4-11d допускает cross-page instruction fetch только для bare `8B 03` в том
  же bounded profile. Проверены second-byte nonpresent/NX fault semantics,
  сохранение CPU/retirement, точные A-update journals, отсутствие operand
  walk/device dispatch и decoder-level контроль отсутствия лишнего fetch.
  `gpt-6.1-sol/high` review одобрило срез без оставшихся P0—P3 замечаний;
  см. [отчёт](STAGE_4_S4-11D_2026-11-05.md), [контракт](../specs/MACHINE_CONTRACT.md#bounded-cross-page-instruction-fetch-s4-11d)
  и [manifest](data/stage4/s4-11d-cross-page-instruction-fetch.json). Полный
  S4-11 остаётся открытым.
- S4-11e добавляет отдельный exact `PagedMovCrossPageWordReadLong64V1` для `66 8B 03` (`MOV AX,[RBX]`). Same-page контроль и crossing read из offset `0xfff` проходят только через две non-contiguous RAM pages; обе walks, права и RAM admission происходят до чтения. Успех меняет AX, сохраняет RAX[63:16], продвигает RIP на 3 и retire-ит один раз. CPL0/CPL3 second-page faults проверены с точным PFEC, A-prefix persistence, без D, CPU/buffer commit и retirement; hole/MMIO/alias и negative profiles покрыты. Для профиля требуется AC=0. Независимое `gpt-6.1-sol/high` review одобрило срез после исправления двух P2 admission findings и P3 рассинхронизации Graphify evidence; оставшихся P0—P3 нет. См. [отчёт](STAGE_4_S4-11E_2026-11-05.md), [контракт](../specs/MACHINE_CONTRACT.md#bounded-cross-page-word-read-s4-11e), [CPU contract](../specs/CPU_CONTRACT.md#exact-paged-word-read-profile-pagedmovcrosspagewordreadlong64v1-s4-11e) и [manifest](data/stage4/s4-11e-cross-page-word-read.json). Родительский S4-11 и ISSUE-054 остаются открытыми.
- S4-11f adds only a separate word-MOV profile: exact `66 8B 03` may start at code-page offsets `0xffe` and `0xfff`, while its operand remains within a separate RAM page. S4-11e V1 retains its no-crossing-fetch restriction. Same-page, PFEC, A/D, no-operand-walk, hole/MMIO, fetch-priority, cross-page-operand rejection, CR0/CR4 pairing and NXE admission are verified; 33 integration, 82 machine unit and 3 focused ISA tests pass. `gpt-6.1-sol/high` review approved after fixing three P2 and three P3 findings; no P0—P3 remain. See [S4-11f report](STAGE_4_S4-11F_2026-11-05.md) and [manifest](data/stage4/s4-11f-cross-page-word-fetch.json). ISSUE-054 and parent S4-11 remain open.
- S4-12a добавляет `Long64ProbeConfig::from_cpu_state`: CR0 и CR4 читаются из
  `LongModeState` с явной ошибкой отсутствующего регистра, CR3 и EFER остаются
  явными входами, а вся существующая validation переиспользуется без defaults,
  masking или mode-transition effects. Snapshot immutable и диагностический.
- S4-12b реализует точный CPU-профиль `0F 22 D8` (`MOV CR3,RAX`) с CPL0,
  reserved-bit validation, CPU-owned CR3 и typed context-change exit после
  единственного retired instruction. Старты на `0xff2–0xfff` отклоняются до
  fetch, а legacy/JIT пути отказываются исполнять CPU-owned CR3. Активная
  machine translation смена не входила в этот CPU-срез. Независимое
  `gpt-6.1-sol/high` ревью приняло срез после пяти раундов и исправления
  найденных дефектов. Результаты записаны в
  [отчёте](STAGE_4_S4-12B_2026-11-05.md) и
  [manifest](data/stage4/s4-12b-mov-cr3.json).
- S4-12c добавляет opt-in bounded CPU-owned CR3 machine quantum: после
  успешного `MOV CR3` проверяется typed effect, строится новый immutable
  translation snapshot, и следующий fetch/operand access использует новый
  root. Доказаны A→B code/data mappings, quantum handoff и precise fetch fault
  после переключения с сохранённым retirement. Профиль ограничен `MOV CR3,RAX`
  и exact `MOV RAX,[RBX]`; stores, PIO и HLT отклоняются. Независимое
  `gpt-6.1-sol/high` одобрило код; синхронизированы evidence и результаты
  Graphify после замечания к манифесту. См. [отчёт](STAGE_4_S4-12C_2026-11-05.md),
  [контракт](../specs/MACHINE_CONTRACT.md#cpu-owned-cr3-translation-handoff-s4-12c)
  и [manifest](data/stage4/s4-12c-mov-cr3-machine.json). Родитель S4-12 остаётся
  открытым для EFER, startup transitions и TLB/invalidation.
- S4-12d добавляет CPU-owned CR0.WP handoff для exact `MOV CR0,RAX` с двумя
  разрешёнными значениями CR0 и следующей supervisor-записи `MOV [RBX],RCX`.
  Машина применяет новый immutable snapshot до следующего poll/fetch/access;
  проверены WP `1→0`, точный fault для `0→1`, STOP/fetch boundaries и отсутствие
  data/D commit при fault. Direct CPU path разделяет #GP для bare MOV CR0 и
  profile rejection для обычной записи вне CPL0, отвергает cross-page stores
  до доступа к данным; только exact LOCK MOV CR0 даёт #UD. Итоговая независимая
  проверка и ссылки: [отчёт](STAGE_4_S4-12D_2026-11-06.md),
  [contract](../specs/CPU_CONTRACT.md#paged-cpu-owned-cr0wp-profile-pagedcpl0cr0wpprobev1-s4-12d),
  [machine contract](../specs/MACHINE_CONTRACT.md#cpu-owned-cr0wp-translation-handoff-s4-12d)
  и [manifest](data/stage4/s4-12d-cr0-wp-machine.json). Полный S4-12 остаётся
  открытым.
- S4-13a добавляет opt-in one-entry pure-translation cache и release comparison
  harness. Три процесса завершили 600 retained paired samples на synthetic
  profiles; результаты включают locality wins и two-page alternating slowdown,
  поэтому production TLB capacity/policy не выбрана.
- S4-14a закрепляет host-controlled lifecycle границу: `clear()` идемпотентен,
  не пишет RAM/A-D, failed replacement не resident, а remap и изменения RW/NX
  наблюдаются только после завершения старого immutable-borrow scope и создания
  нового cache. Guest invalidation и CPU hooks не добавлены.
- S4-14b добавляет standalone feature-independent `INVLPG [RBX]` CPU profile с
  typed effect; feature-gated consumer очищает one-entry experimental cache до
  следующего lookup. Проверен `Walk → Hit → INVLPG → consume → Walk → Hit`,
  включая full-clear при другой resident page и сохранение cache при fault/no-op.
  S4-14a immutable-memory boundary сохраняется: tracked A/D, remap,
  huge pages, global/PCID, CR3 invalidation, JIT и production TLB не заявлены.
  См. [report](STAGE_4_S4-14B_2026-11-06.md),
  [CPU contract](../specs/CPU_CONTRACT.md#standalone-invlpg-profile-invlpglong64v1-s4-14b)
  и [machine contract](../specs/MACHINE_CONTRACT.md#bounded-invlpg-effect-consumption-s4-14b).
- S4-21a добавляет синхронную bounded diagnostic vCPU session над одним
  `LongModeState` и `MachineBus`: runnable/paused/stopped/terminal states,
  finite instruction budget, сохранение `rflags_defined`, STOP callback и
  отдельные fault/unsupported/backend exits. Полный vCPU lifecycle не закрыт.
- S4-22a добавляет caller-driven `VirtualClock` и bounded RDTSC boundary:
  checked `u128` conversion, atomic overflow rejection, explicit time advance,
  сохранение `rflags_defined`, TSD/CPL/fetch checks и отсутствие auto-advance.
  RDTSCP, MSR/CPUID, HLT progression, IRQ/deadlines и host-clock coupling не
  реализованы.
- S4-22b добавляет один явный `VirtualClock` владельцу `PagedVcpuSession`;
  RX deadlines и dispatch теперь используют единый наносекундный домен.
  Advancement явный и допустим во время guest HLT без wakeup, но отклоняется
  при host pause/stop/terminal. Reset сохраняет время и частоту; overflow
  атомарен. Частота runner диагностическая и не публикуется через CPUID/MSR.
  См. [S4-22b report](STAGE_4_S4-22B_2026-11-07.md).
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
- S4-23d публикует через `PagedVcpuSession` queue-local generation, read-only
  ближайший RX deadline и generation-aware schedule/cancel/reschedule. Stale
  generation отклоняется раньше lifecycle admission; HLT, pause, STOP и terminal
  не принимают мутации. Подробности в [отчёте S4-23d](STAGE_4_S4-23D_2026-11-07.md)
  и [manifest](data/stage4/s4-23d-paged-rx-control.json). HLT wakeup и host wait
  остаются открытыми требованиями S4-23.

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

## S4-21c — bounded paged owner (accepted)

The fixed-image paged owner implements caller-driven STOP, pause/resume and
replacement reset with checked RX generation invalidation. The 120 od-machine
unit tests, 21 paged-quantum tests and 7 UART RX tests passed. Independent
Sol/high review found no P0–P3 issues. Evidence and limits are in the
[S4-21c report](STAGE_4_S4-21C_2026-11-07.md). S4-21 and Stage 4 remain open;
runner completion/cancellation is not implemented.

## S4-21d — runner completion and deterministic cancellation (accepted)

The fixed-image runner uses the paged owner and requires an expected nonce,
three retired instructions and exact UART output before reporting guest
completion. STOP and work limit are distinct. Independent review passed; the
README error wording was clarified. See [report](STAGE_4_S4-21D_2026-11-07.md).
S4-21 and Stage 4 remain open; VMHost/OS signal integration belongs to S4-41.

## S4-25a — fixed-edge Local APIC register core

The bounded `LocalApic` device exposes the selected xAPIC MMIO register
subset and keeps fixed-edge request, priority eligibility, acknowledge and EOI
state. Two focused state/bus tests and the `od-machine` all-targets check,
formatting and diff validation passed; independent Sol/high review closed its
three test-coverage findings. This does not close S4-25: the version register,
LVT, APIC-base MSR, default board wiring, IOAPIC, timer and CPU interrupt
delivery remain open. See the [slice report](STAGE_4_S4-25A_2026-11-07.md)
and [evidence manifest](data/stage4/s4-25a-local-apic-core.json).

## S4-25b — CPU-owned IA32_APIC_BASE enable

The bounded opt-in `ApicBaseCpu` executes bare `RDMSR`/`WRMSR` for index
`0x1B` in Long64/CPL0, with reset value `0xFEE00900` and fixed-base global
disable value `0xFEE00100`. Successful writes return a typed CPU effect; the
machine helper applies it to the standalone xAPIC before returning. Same-value
writes preserve APIC state; global disable resets it and blocks MMIO, and
re-enable requires SVR software enable. Three focused ISA tests, four Local
APIC tests, both package all-targets checks, formatting and diff validation
passed. Independent Sol/high review fixed and confirmed closure of two P2
boundary findings. This does not add the profile to `PagedVcpuSession` or
default board wiring, and does not implement relocation, x2APIC, CPU interrupt
delivery or HLT wakeup. See [report](STAGE_4_S4-25B_2026-11-07.md) and
[evidence](data/stage4/s4-25b-apic-base.json). Parent S4-25 and Stage 4 remain
open.

## S4-25c — xAPIC Fixed self-IPI

The Local APIC now accepts an aligned Dword write of the bounded Fixed Self
edge command to ICR low, stores ICR high destination byte without dispatch,
and records the vector in IRR on ICR low. Tests cover readback, high-only
writes, repeat/coalescing behavior, priority, acknowledge/ISR/EOI, software
and global disable, reset, and failure atomicity. Six focused APIC tests,
`od-machine` all-targets check, format/diff validation and independent
Sol/high review passed. This is synchronous internal one-CPU device delivery
only; a written read-only Delivery Status bit is ignored and readback remains
Idle. The P2 review finding on this behavior was fixed and re-reviewed. It does
not claim CPU handler acceptance, APIC bus transmission,
external destinations, IOAPIC or HLT wakeup. See [report](STAGE_4_S4-25C_2026-11-07.md)
and [evidence](data/stage4/s4-25c-xapic-self-ipi.json). Parent S4-25 and Stage 4
remain open.

## S4-25d — bounded CR8 ↔ xAPIC TPR

An opt-in Long64/CPL0 exact-encoding profile connects `MOV RAX, CR8` and
`MOV CR8, RAX` to the standalone Local APIC TPR. It maps CR8's low nibble to
TPR priority class, applies writes before the machine helper returns, and
demonstrates a pending self-IPI becoming eligible when TPR drops below its
priority. Instruction fetch enforces canonical48. Record final focused tests,
checks, Graphify counts, and Sol/high review outcome in the
[slice report](STAGE_4_S4-25D_2026-11-07.md) and
[evidence](data/stage4/s4-25d-cr8-tpr.json). The caller must serialize APIC
operations across the complete helper call. General CR8 decoding, CPU handler
delivery, session/JIT integration and default board wiring are not included;
parent S4-25 and Stage 4 remain open.

## S4-26a — fixed-edge IOAPIC route

The opt-in `od-machine` IOAPIC exposes a single programmable INTIN4 route to
physical Local APIC ID 0. Dword IOREGSEL/IOWIN accesses configure the route's
vector and mask; only active-high Fixed edge mode is supported. A low-to-high
line transition forwards to the Local APIC IRR, where the existing TPR,
acknowledge and EOI model determines eligibility and service. Masked edges are
ignored without replay on unmask, and read-only Delivery Status/Remote IRR
writes do not alter stored state. Four IOAPIC tests, nine Local APIC regression
tests, `od-machine` all-targets check, fmt/diff and independent Sol/high review
passed. Only one edge route is modeled; level routing, other entries, CPU
handler delivery, board wiring and Linux discovery are not established. See
the [slice report](STAGE_4_S4-26A_2026-11-07.md) and
[evidence](data/stage4/s4-26a-ioapic-fixed-edge.json). Parent S4-26 and Stage 4
remain open.

## S4-26b — bounded fixed-level IOAPIC route

The single INTIN4 route now supports active-high Fixed level mode with Local
APIC TMR and IOAPIC Remote IRR state, matching EOI and atomic retrigger while
the line remains asserted. A shared owner lock makes route acceptance and EOI
transitions atomic. Independent resets preserve the other controller and veto
without mutation during outstanding Remote IRR; pair reset is explicit. The
APIC-base helper publishes CPU state only after its APIC transition succeeds.
Eleven IOAPIC tests, ten Local APIC tests, the `od-machine` all-targets check,
formatting, diff validation, Graphify and repeat Sol/high review passed. The
review's initial P2 reset regression was fixed and confirmed closed. This does
not implement guest CPU handler delivery or board registration. See the
[slice report](STAGE_4_S4-26B_2026-11-07.md) and
[evidence](data/stage4/s4-26b-ioapic-level.json). Parent S4-26 and Stage 4
remain open.

## S4-27a — caller-driven APIC interrupt entry

The diagnostic vCPU session now selects one APIC-eligible vector, runs the
existing Long64/CPL0 IDT-entry helper on a CPU candidate through flat identity
RAM, acknowledges that exact vector only after successful entry, and runs the
handler with a one-instruction budget. A fetch/execute fault or unsupported
handler instruction does not undo successful entry or acknowledgement. The
end-to-end fixture checks the five-qword frame,
IF clearing, RAM sentinel read, ISR state and Remote IRR retention until EOI.
Blocked requests remain pending; an aborted frame can retain its committed
prefix and makes the session terminal without acknowledgement or retry. Nine
session tests, ten Local APIC tests, seven public API tests, the ISA stack-store
failure canary, `od-machine` all-targets check, formatting, Graphify and
independent Sol/high review passed. This is a caller-serialized diagnostic
flat identity path; it does not add paged delivery, automatic event polling,
IRET/guest EOI, HLT wakeup, JIT or board registration. See the
[slice report](STAGE_4_S4-27A_2026-11-07.md) and
[evidence](data/stage4/s4-27a-apic-cpu-entry.json). Parent S4-27 and Stage 4
remain open.

## S4-27b — guest EOI, same-CPL IRETQ and resume

The diagnostic handler now issues a guest Dword EOI write through the existing
MachineBus MMIO path. An explicit same-CPL0 IRETQ profile consumes the five-
qword frame; the next caller-driven run executes the interrupted instruction.
Tests cover all 16 original RSP alignments, restored CPU state, deasserted EOI,
held-line request acceptance into IRR at EOI with CPU delivery at the next caller boundary, and IRETQ fetch/frame/CS/SS
read aborts after EOI without CPU commit or retry. Twelve diagnostic-session
tests, eight public API tests, ten Local APIC tests, 25 `od-isa::iretq` tests,
all-targets check, formatting, Graphify and independent Sol/high review passed. This remains a
caller-driven flat identity diagnostic profile without paged IRETQ, automatic
polling, HLT wakeup, JIT or board registration. See the
[slice report](STAGE_4_S4-27B_2026-11-07.md) and
[evidence](data/stage4/s4-27b-guest-eoi-iretq.json). Parent S4-27 and Stage 4
remain open.

## S4-28a2 — shared vCPU and APIC timer clock

The opt-in one-shot APIC timer now observes the same `VirtualClockHandle` as
`DiagnosticVcpuSession` RDTSC and caller-driven session advancement. Shared
timer configurations reject APIC-local time advancement; standalone S4-28a1
operation is retained. One integration case proves RDTSC and timer Current
Count/expiry agree at 2 ns and 4 ns. A unique capability prevents a second
session from claiming advancement, and due dispatch remains queued while the
session is Paused or Stopped. `od-machine` all-targets check, all 190 unit
tests, formatting and diff checks passed. Astra/high architecture review and
repeat Sol/high code review passed; the reviewer confirmed all three P2
findings were closed.
No automatic event pump, CPU delivery, HLT progression or Linux timer behavior
is claimed. See the [S4-28a2 report](STAGE_4_S4-28A2_2026-11-07.md) and
[evidence](data/stage4/s4-28a2-shared-clock.json).

## S4-28a3 — shared timer wake from HLT

`DiagnosticHaltWait` carries the unique session clock capability and allows
explicit time advancement only while Waiting. A caller then dispatches the
one-shot APIC event and polls the existing maskable interrupt wake path. The
APIC-pair → clock lock order keeps the timer gate, queue removal and IRR post
atomic; wake/STOP closes the gate before callbacks and `into_running` reopens it
after committed entry. The focused integration case covers overflow atomicity,
two timer expiries around HLT entry, pending-event preservation, and RDTSC after
IRETQ. Architecture review and repeat Sol/high code review passed; the review's
P2 API-export finding was fixed. All 191 package tests, all-targets check,
formatting and diff checks passed. No automatic scheduler, host-time source,
periodic timer or Linux acceptance is claimed. See the
[S4-28a3 report](STAGE_4_S4-28A3_2026-11-07.md) and
[evidence](data/stage4/s4-28a3-hlt-timer.json).

## S4-34b — KVM identity and repeatability (physical run complete; review pending)

The isolated `codex` worktree has a repeatable KVM method runner that records
the configured guest CPUID model and host microcode, retains every invocation,
and rejects malformed CPU identity, unstable outcomes, injection trace/UART
mismatches, runner source/binary changes, or image hash changes. Forty-one offline
builder tests pass. On the Mini, three repeats of each of the three existing
cases were stable after masking only per-CPU APIC identifiers in the CPUID
identity comparison; raw CPUID models/hashes remain recorded for every
invocation. All cases passed their exact expected exit and UART checks, and
runner/image integrity hashes matched. S4-34b awaits independent review;
S4-34 remains open because architectural system cases are still missing. See
the [S4-34b report](STAGE_4_S4-34B_2026-11-14.md) and
[repeatability evidence](data/stage4/s4-34b-kvm-repeatability.json).

## S4-42a — translation micro-benchmark (first measurement; S4-42 open)

Подсрез измеряет только трансляцию Long64 4 KiB на уровне API `od-machine`:
uncached `probe_long64_4k` против единственного существующего cached-пути,
экспериментального capacity-one `TranslationProbeCache`
(feature `experimental-translation-probe-cache`; полноценного TLB в коде нет).
Linux boot, JIT, CPU-интерпретатор, IRQ/timer и Mac mini в этот подсрез не
входят; это не вывод про «ускорение MMU».

**Что измерено** (`od-bench translation-micro`, `tools/bench/src/translation_micro.rs`).
Фикстура: 32 MiB RAM, пять 16 MiB регионов с разными правами листа
(user RWX, user read-only, supervisor-only, user NX, not-present), CR0=`0x80010011`
(PG|WP), CR4=`0x20`, EFER=`0xd00` (NXE). Рабочие наборы 16, 256 и 4096 страниц
(64 KiB, 1 MiB, 16 MiB). Порядок: sequential (4 обращения на страницу
подряд, offsets 0/8/64/4088) и random (тот же набор, детерминированная
перестановка). Пути: Read/Write/Fetch в ring3 и Read ring0 к supervisor-странице
(успешные права); пять fault-путей (not-present, user-denied, write к read-only
из ring3 и из ring0 при WP, fetch NX), только sequential; cross-page — 8-байтовое
обращение с offset `0xffc`, моделируется двумя вызовами probe (первый и
последний байт). Всего 15 сценариев × 3 набора = 45 ячеек. В каждой ячейке
15 повторов батча (батч ≥ 50 мс обеих сторон, число эпизодов калибруется), порядок
uncached/cached чередуется по повторам; cache — свежий на батч, между эпизодами
батча сохраняется. Число обращений в ячейке — `accesses`, число вызовов probe —
`probes` (cross-page: 2 probe на обращение).

**Correctness.** Перед замером для каждого запроса сравниваются три результата:
uncached probe, cached путь (два эпизода) и независимый cache-free обход таблиц
`oracle` в самом бенчмарке (собственный обход: индексы, AND по U/W, OR по NX, WP/NXE, своя сборка
PFEC; общие с walker только `PhysicalMemory::read_u64` и
`AccessContext::page_privilege()`; reserved bits и PS не проверяются, в
фикстуре их нет); cache hit обязан равняться полному `Translation4K` обхода.
После этого каждый таймированный батч обязан дать checksum всех физических
адресов/PFEC, равный oracle-checksum; при расхождении весь прогон завершается
ошибкой без чисел. Хеш RAM фикстуры до и после замера совпал. Результат:
все 45 ячеек приняты, все батчи совпали. Unit-тесты (4 новых) проверяют литеральные
адреса и PFEC (4/5/7/21/3), детерминизм перестановки и отклонение неверной
checksum.

**Числа** — медиана (min–max) по 15 повторам, нс на обращение; `C/U` — медиана
парных отношений cached/uncached (>1 — cached медленнее). Rights-ok (Read/Write/Fetch/ring0)
по ячейкам неотличимы друг от друга, поэтому показан Read (остальные в JSON).

| Сценарий | Порядок | Страниц | Hit/walk/fault (на эпизод) | Uncached | Cached | C/U |
|---|---|---|---|---|---|---|
| read-user | seq | 16 | 48/16/0 | 29.1 (29.0–30.0) | 10.9 (10.9–11.0) | 0.37 |
| read-user | seq | 256 | 768/256/0 | 29.0 (28.9–29.1) | 10.9 (10.8–11.0) | 0.38 |
| read-user | seq | 4096 | 12288/4096/0 | 29.1 (29.0–29.6) | 10.9 (10.8–11.3) | 0.37 |
| read-user | random | 16 | 2/62/0 | 29.2 (29.0–29.7) | 33.2 (33.1–33.9) | 1.14 |
| read-user | random | 256 | 3/1021/0 | 29.0 (28.9–29.3) | 33.7 (33.6–34.4) | 1.16 |
| read-user | random | 4096 | 1/16383/0 | 29.1 (29.0–30.3) | 33.8 (33.7–35.6) | 1.16 |
| fault-not-present | seq | 256 | 0/1024/1024 | 28.0 (27.9–28.4) | 31.6 (31.4–32.1) | 1.13 |
| fault-user-denied | seq | 256 | 0/1024/1024 | 29.5 (29.5–30.9) | 32.7 (32.6–35.3) | 1.11 |
| fault-write-readonly-ring3 | seq | 256 | 0/1024/1024 | 29.7 (29.5–30.4) | 33.1 (33.0–34.1) | 1.12 |
| fault-write-readonly-ring0-wp | seq | 256 | 0/1024/1024 | 29.5 (29.5–40.5) | 33.1 (33.0–33.7) | 1.12 |
| fault-fetch-nx | seq | 256 | 0/1024/1024 | 29.5 (29.5–31.3) | 33.0 (33.0–33.3) | 1.12 |
| cross-page-read-user | seq | 16 | 14/16/0 из 30 probe | 59.0 (58.8–59.3) | 39.8 (39.8–40.1) | 0.68 |
| cross-page-read-user | seq | 256 | 254/256/0 из 510 | 58.5 (58.4–59.6) | 37.5 (37.4–37.8) | 0.64 |
| cross-page-read-user | seq | 4096 | 4094/4096/0 из 8190 | 58.5 (58.4–58.9) | 37.4 (37.3–37.7) | 0.64 |
| cross-page-read-user | random | 16 | 0/30/0 | 58.9 (58.7–59.1) | 69.2 (69.0–70.1) | 1.18 |
| cross-page-read-user | random | 256 | 2/508/0 | 58.5 (58.4–58.8) | 68.7 (68.5–69.7) | 1.17 |
| cross-page-read-user | random | 4096 | 1/8189/0 | 58.5 (58.3–58.9) | 68.7 (68.7–69.7) | 1.18 |

Полная таблица всех 45 ячеек, все 15 сырых повторов, calibration batches,
счётчики (включая число прочитанных table entries: 4 на uncached probe) и
checksum лежат в [s4-42a-translation-micro.json](data/stage4/s4-42a-translation-micro.json).

**Разброс.** Относительный размах (max−min)/median по повторам для 90 рядов
(uncached и cached в 45 ячейках) — в основном 1–8 %; единственный выброс —
uncached `fault-write-readonly-ring0-wp`/256 (37 %, один повтор до 40.5 нс
при медиане 29.5; медиана и остальные повторы не затронуты). Медианы
стабильны; отдельные повторы C/U отклоняются до ±0.08 (например
write-user/random/16: min 1.059 при медиане 1.142; fault-fetch-nx/256: 1.061
при 1.119).

**Что показывают числа (и только это).**
- На последовательных обращениях capacity-one cache дал ≈10.9 против ≈29 нс
  на обращение (C/U 0.37). Доля hit 75 % задана самой фикстурой (4 обращения
  на страницу подряд: 3 из 4 попадают в ту же страницу), и 0.37 — арифметика
  этой доли, а не свойство кэша вообще. Из данных (random-ряд даёт цену промаха
  ≈33.7 нс, 0.75·h + 0.25·33.7 ≈ 10.9) цена hit получается ≈3.3 нс, цена промаха
  ≈33.7 нс (оценка по формуле, прямо не измерена). Зависимость от доли hit
  (1/2/4/16 обращений на страницу) в этом подсрезе НЕ измерена; это следующий
  подсрез. Эффект не зависит от размера рабочего набора.
- На cross-page sequential cached быстрее в ≈1.5× (≈37–40 против ≈58.5 нс).
  Доля hit ≈50 % возникает потому, что первая страница запроса i+1 совпадает
  со второй страницей запроса i; это не результат split-пути.
- На случайных обращениях cache не даёт выигрыша: hit почти нет (1–3 из
  64–16384), cached на 14–18 % медленнее uncached (≈+4 нс на промах: проверка
  тега и перезапись записи). То же для random cross-page (+17–18 %).
- Fault-пути не кэшируются по контракту (0 hit); cached на 11–13 % медленнее
  из-за той же проверки тега.
- Права (Read/Write/Fetch, ring0/ring3) на время walk не влияют (28.7–29.2 нс
  медианы всех успешных путей); fault-пути стоят столько же, сколько успешный walk
  (28.0–29.7 нс).
- Uncached время не зависит от размера рабочего набора (16–4096 страниц дают
  одно и то же): таблицы страниц для 4096 страниц занимают ~32 KiB листов и
  помещаются в кэш хоста, так что этот бенчмарк не измеряет промахи host-кэша
  при больших рабочих наборах и не различает 4 KiB host/guest страницы.
- Это не доказывает полезность TLB: capacity-one cache — отдельный
  эксперимент без инвалидации и A/D; многоэлементный TLB, который мог бы
  помочь на random-наборах, не измерялся (его нет). Усложнение TLB по этим
  данным не обосновано и не опровергнуто.

**Среда и воспроизведение.** Apple M3 Max (14 логических CPU), macOS 26.7.1,
aarch64-apple-darwin, `rustc 1.99.0 (b940084d7 2026-09-28)`, профиль `release`
(стандартный cargo release: opt-level 3, без LTO; в workspace нет `[profile]`,
`RUSTFLAGS` не задан), host page 16384 B, guest page 4096 B, питание от
AC. Источник: commit `9d96348` (feat: S4-42a translation micro-benchmark),
дерево чистое, `source_sha256` `f9001fd9…d213`; sha256 бинаря
`6652218e1ec3d8f42bedfa0c2610c5452ce097ce2db40e9854389b05a4fd082a`; sha256
RAM фикстуры `3511e450…1ddc`, sha256 профиля запросов `0b428c18…fe2`.
Команда: `cargo build --release -p od-bench && target/release/od-bench
translation-micro --process-run 1 --output <path>`; прогон длится ≈145 с.
Выполнен один процесс (`--process-run 1`); повторы внутри процесса — 15.

**Ограничения.** Измерение на ноутбуке (M3 Max), не на Mac mini; один процесс,
без межпроцессных повторов и без изоляции от фоновой нагрузки; нет
JIT, Linux, IRQ/timer, memory consumption; cross-page — модель из двух probe,
а не отдельный split-путь CPU; tracked A/D-путь (`access_long64_4k_ram`) и
2 MiB страницы не измерялись; весь замер — pure probe без доступа к
операнду. Нет вывода о скорости Linux boot или ускорении MMU. S4-42 остаётся
открытым: не сделаны JIT cold/warm, ограниченный Linux boot, IRQ/timer,
Mac mini и повторные процессы.

## S4-42b — зависимость стоимости трансляции от доли hit (S4-42 open)

Подсрез отвечает на «не сделано» из S4-42a: вместо оценки по формуле измерить
цену hit и цену промаха по серии с разной долей hit. Те же рамки, что в S4-42a:
только API трансляции `od-machine` (uncached `probe_long64_4k` против
экспериментального capacity-one `TranslationProbeCache`); Linux boot, JIT,
CPU-интерпретатор, IRQ/timer и Mac mini не входят. Ядро MMU/TLB не менялось.

**Что измерено** (`od-bench translation-hit-share`, тот же модуль
`tools/bench/src/translation_micro.rs`, схема `od-bench-translation-hit-share-v1`).
Фикстура, CR0/CR4/EFER, oracle, правила калибровки и повторов те же, что в S4-42a.
Сценарий: sequential Read ring3 по user-RWX региону, рабочий набор 256 страниц
(1 MiB), 1/2/4/16 обращений подряд на одну страницу (8-байтовые, offsets
равномерно по странице и кратны 8, границу страницы не пересекают; для 4
обращений offsets 0/1024/2048/3072 вместо 0/8/64/4088 из S4-42a). Доля hit
берётся не из формулы, а из preflight-счётчиков (hit/probes): 0, 0.5, 0.75,
0.9375 (для capacity-one cache это ровно (k−1)/k). 4 ячейки, в каждой 15
повторов батча (≥ 50 мс обеих сторон), порядок uncached/cached чередуется по
повторам, cache свежий на батч. Один процесс (`--process-run 1`), один полный
прогон.

**Correctness.** Как в S4-42a: перед замером для каждого запроса uncached probe,
cached путь (два эпизода) и независимый cache-free обход `oracle` обязаны
совпасть, cache hit обязан равняться полному `Translation4K`; каждый таймированный
батч сверяется с oracle-checksum; хеш RAM фикстуры до и после совпал
(`3511e450…1ddc`, тот же, что в S4-42a). Все 4 ячейки приняты, все батчи совпали.
Новые unit-тесты (2): точные счётчики hit/walk серии (hit = 256·(k−1), walk = 256,
ноль fault) вместе с oracle-проверкой и точное восстановление цены hit/промаха
линейной регрессией на синтетических данных.

**Числа** — медиана (min–max) по 15 повторам, нс на обращение; `C/U` — медиана
парных отношений cached/uncached.

| Обращений на страницу | Доля hit | Hit/walk (на эпизод) | Uncached | Cached | C/U |
|---|---|---|---|---|---|
| 1 | 0 | 0/256 | 30.31 (30.28–30.84) | 35.48 (35.40–35.93) | 1.17 (1.15–1.19) |
| 2 | 0.5 | 256/256 | 30.34 (30.28–31.86) | 19.37 (19.33–19.66) | 0.64 (0.61–0.65) |
| 4 | 0.75 | 768/256 | 30.34 (30.27–30.89) | 11.24 (11.22–11.33) | 0.37 (0.36–0.37) |
| 16 | 0.9375 | 3840/256 | 30.37 (29.81–31.36) | 5.24 (5.13–5.44) | 0.172 (0.172–0.177) |

Uncached от доли hit не зависит (30.31–30.37 нс, наклон 0.06 нс на единицу доли).
Cached падает линейно: добавление hit заменяет ≈35.5 нс промаха на ≈3.2 нс hit.
Все 15 повторов, счётчики и checksum — в
[s4-42b-hit-share.json](data/stage4/s4-42b-hit-share.json).

**Цена hit и промаха** (метод: МНК `cached_ns = miss + (hit − miss)·share` по 4
ячейкам; значение при доле 0 — цена промаха, при доле 1 — цена hit; цена —
стоимость обращения целиком, включая цикл и свёртку checksum бенчмарка).

| Величина | Регрессия по медианам | Разброс: 15 регрессий по повторам (медиана, min–max) | Прямая оценка | S4-42a (формула) |
|---|---|---|---|---|
| Цена hit, нс | 3.21 | 3.20 (3.09–3.37) | 3.22 (из ячейки 16 при прямой цене промаха) | ≈3.3 |
| Цена промаха, нс | 35.48 | 35.49 (35.42–35.87) | 35.48 (ячейка 1 обращение/страница, доля 0, измерена напрямую) | ≈33.7 |

R² регрессии по медианам 0.99999, максимальная невязка 0.03 нс: модель
«стоимость = доля·hit + (1−доля)·промах» на этих данных выполняется. Промах в
cached-пути стоит на ≈5.2 нс больше uncached walk (35.5 против 30.3); hit — в ≈9
раз дешевле walk (3.2 против 30.3).

**Совпадение с оценкой S4-42a.** Цена hit совпала (3.21 против ≈3.3, в пределах
разброса повторов). Цена промаха НЕ совпала с 33.7: получилось 35.5, то есть на
≈1.8 нс (≈5 %) больше, и это за пределами разброса повторов внутри процесса (35.4–35.9).
Расхождение, скорее всего, процессное: в этом процессе uncached walk тоже стал
медленнее на ≈1.3 нс (30.3 против 29.0 в S4-42a, +4.6 %) и cached sequential/4
чуть выше (11.24 против 10.9), то есть весь уровень сдвинут примерно на 5 %;
разница «промах минус uncached» (5.2 против 4.7 нс) близка. Но
это объяснение не проверено: процесс один, повторов между процессами нет, поэтому
отделить межпроцессный дрейф от настоящей разницы между sequential-промахом
(этот подсрез) и random-промахом (S4-42a, 3 hit на 1024) нельзя. Вывод для
S4-42: оценка S4-42a для hit подтверждена прямым измерением, для промаха занижена
на ≈5 % в пределах вероятного межпроцессного дрейфа; для надёжного различия
нужны повторные процессы.

**Что показывают числа (и только это).**
- Выигрыш capacity-one cache целиком определяется долей hit: при 0 % hit cached
  на ≈17 % медленнее uncached (C/U 1.17), безубыточность около 16 % hit
  (≈1 hit на 6–7 обращений, из линейной модели), при 94 % hit cached в ≈5.8 раза
  быстрее (5.2 против 30.4 нс).
- Эффект не зависит от размера рабочего набора (S4-42a), в этом подсрезе
  использован один набор 256 страниц.
- Это не доказывает полезность TLB: единственный cache — capacity-one без
  инвалидации и A/D; доля hit в реальном Linux-коде не измерена и этим
  бенчмарком не определяется.

**Среда и воспроизведение.** Apple M3 Max (14 логических CPU), macOS 26.7.1,
aarch64-apple-darwin, `rustc 1.99.0 (b940084d7 2026-09-28)`, профиль `release`
(стандартный, без LTO, `RUSTFLAGS` не задан), питание от AC. Источник: commit
`9acb857` (feat: S4-42b hit-share series), дерево чистое, `source_sha256`
`12ea98bd…90d2`; sha256 бинаря
`8b4611d6015ee003ba59f4564dc18600fdb65a78aea702e3063f6aecebaa3f5c`; sha256 RAM
фикстуры `3511e450…1ddc`, sha256 профиля запросов `fcd22115…5fc4`. Команда:
`cargo build --release -p od-bench && target/release/od-bench translation-hit-share
--process-run 1 --output <path>`. Проверки: `cargo test -p od-bench`,
`cargo check --workspace --all-targets` пройдены; `cargo fmt --check` показывает
только известные расхождения `storage_abi_v1.rs:139` и
`tools/image-tool/tests/cli.rs:109`.

**Ограничения.** Ноутбук (M3 Max), не Mac mini; один процесс, нет межпроцессных
повторов и изоляции от фоновой нагрузки; доля hit задана длиной серии на странице,
реальное распределение обращений не моделируется; 4 точки (1/2/4/16) и один
рабочий набор; цены hit/промаха включают накладные расходы цикла и checksum
бенчмарка; только Read ring3 без A/D и без 2 MiB страниц. S4-42 остаётся
открытым: не сделаны 2 MiB страницы, tracked A/D-путь, JIT cold/warm,
ограниченный Linux boot, IRQ/timer, Mac mini и повторные процессы.
