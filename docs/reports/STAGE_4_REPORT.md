# Отчёт этапа 4

## Текущий статус

**Состояние на 2026-11-15 (UTC): этап 4 частично закрыт, не завершён.**
Формулировка итога: Linux userspace локально подтверждён на пути
интерпретатора под CPUID-профилем stage4a для узкого сценария `/init`; S4-41,
S4-42, S4-43 приняты с ограничениями (родитель S4-42 по плану остаётся
открытым); S4-44 открыт (часть NOT RUN); S4-45 выполнен как независимое ревью
Opus, внешние модели gpt-6.1-sol / gpt-6-astra недоступны в среде; S4-46 —
частичный итог ([раздел](#s4-46--итог-этапа-4-частичный)).

**Что доказано.**
- S4-39j: два холодных зачётных прогона `stage4a --user-mode` без
  `--diagnostic-past-c7` дали `PASS`, `counted: true`, run ID
  `9d34785cdd85762428ba69873dd543700d87a50af5391e49d1728e79709b92c2`, отчёты
  побайтно идентичны (см. [Linux direct boot](#linux-direct-boot-s4-38s4-39)).
  Сценарий: nonce `OD_NONCE=s4-38l4a` ровно строкой в COM1, `exit(0)` PID 1 и
  штатная паника ядра, журнал syscall 83,165,257,0,3,1,39,35,60, хеши входов
  vmlinuz / initramfs / манифеста ACPI совпали.
- S4-40: негативы гейта не дают ложного `PASS` (с пробелами, см.
  [S4-40](#s4-40--негативы-гейта-с-пробелами)).
- S4-41a/41b: lifecycle в `od-runner linux-direct` и подпроцесс в VMHost
  (с ограничением, см. [S4-41](#s4-41--lifecycle-в-runner-и-vmhost-закрыт-с-ограничением)).
- S4-42a/b/c и S4-43a/b/c: микробенчмарк трансляций и ограниченный churn/reset
  (разделы ниже, S4-42 и S4-43 отдельно).
- S4-44a/b: локальный воспроизводимый набор зелёный
  (`overall_local` = `passed`), полный S4-44 открыт.
- S4-39h implementation slice (2026-11-15): CPL3 external IRQ now switches
  through `TSS.RSP0`; the board loop restarts after delivery and recalculates
  CPL before executing the handler. Independent review and focused ISA/board
  tests passed; this is modeled-path evidence, not a live guest run.

**Что НЕ доказано.**
- Live прерывания и таймер в CPL3 на Linux-госте не проверены. Синтетический
  board integration test S4-39h подтверждает IRQ → CPL0 handler → IRETQ → CPL3
  продолжение; исторический run_failed не установлен как ошибка CPL3 и не
  локализует отказ C/run-0001 из S5-07.
- Профиль stage4a — без APIC и без SSE/SSE2 (Linux работает на 8259); работа
  под другим CPUID-профилем и с APIC не подтверждена.
- Nonce фиксированный (`s4-38l4a`), а не свежий на прогон.
- JIT на Linux не запускался (0 %); подтверждён только путь интерпретатора.
- Гейт принимает `budget_exhausted` как конец прогона (не `user_exit`), не
  проверяет `exit_kind`, `cpuid_profile` и timer IRQ.
- Pause/resume для Linux-платы нет; control v2 не несёт путь к ядру и не
  запускает Linux-профиль; STOP по control v2 не подключён.
- Правило зачётности `counted` для stage4a введено решением пользователя в день
  PASS; независимое ревью самого этого решения не записано.
- Vmlinuz и initramfs в репозитории не хранятся; подлинность ядра не
  подтверждена (S4-35: проверен content hash). Воспроизведение без этих
  артефактов невозможно (см. `docs/testing/MACHINE.md`, раздел «Воспроизведение
  Linux direct boot (S4-39j)»).
- S4-44 открыт: Linux boot/`linux-direct-gate` в наборе не запускались, сборка
  guest/firmware artifacts, VMHost smokes, runner stress mode,
  `--reboot-cycles`, явный лимит guest RAM, лимит executable cache — NOT RUN;
  Mac mini bare-metal blocked.

Остаточные проблемы с владельцами и следующими шагами — в
[KNOWN_ISSUES](../KNOWN_ISSUES.md) (записи ISSUE-090…ISSUE-099). Отклонения от
плана — пометки в [STAGE_4.md](../plans/STAGE_4.md). Разделы: Linux direct boot
(S4-38/S4-39), S4-40, S4-41, S4-42a/b/c, S4-43a/b/c, S4-44a/b, S4-45, S4-46 — ниже.

### Историческая запись статуса на 2026-11-05 (сохранена без изменений)

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

## S4-34b — KVM identity and repeatability (accepted after independent review)

The isolated `codex` worktree has a repeatable KVM method runner that records
the configured guest CPUID model and host microcode, retains every invocation,
and rejects malformed CPU identity, unstable outcomes, injection trace/UART
mismatches, runner source/binary changes, or image hash changes. Forty-one offline
builder tests pass. The slice was accepted after independent `gpt-6.1-sol/high`
review and merged to `main` in `1203ba7`. On the Mini, three repeats of each of the three existing
cases were stable after masking only per-CPU APIC identifiers in the CPUID
identity comparison; raw CPUID models/hashes remain recorded for every
invocation. All cases passed their exact expected exit and UART checks, and
runner/image integrity hashes matched. Host reboot repeatability remains
unverified. S4-34 remains open because architectural system cases are still missing. See
the [S4-34b report](STAGE_4_S4-34B_2026-11-14.md) and
[repeatability evidence](data/stage4/s4-34b-kvm-repeatability.json).

## Linux direct boot (S4-38/S4-39)

- **Уточнение S6-38.** По D5 профиль CPL3 допускает qword POP из стека в регистр через существующий исполнитель. [ISSUE-551 и проверки](STAGE_6_REPORT.md#s6-38--pop-регистра-в-cpl3).

- **Уточнение S6-21.** По D5 добавлена форма `LOCK AND m64, imm8` для остановки в `copy_process`. Она использует общий атомарный AND профиля CPL0. [ISSUE-550 и проверки](STAGE_6_REPORT.md#s6-21--lock-and-qword-с-imm8).

- **Уточнение S6-38.** По D5 согласован допуск точной `ENDBR64` в CPL3. Он использует существующий NOP при отключённом CET IBT. [ISSUE-548 и проверки](STAGE_6_REPORT.md#s6-38--endbr64-в-профиле-cpl3).

- **Уточнение S6-21.** По D5 согласована поддержка `LOCK SUB m32, imm32` и `LOCK SUB m64, imm8` в `long_decode.rs` и `execute.rs`. Обе формы используют существующий атомарный API гостевой памяти в CPL0. [ISSUE-547, ISSUE-549 и проверки](STAGE_6_REPORT.md#s6-21--атомарный-lock-sub-с-непосредственным-операндом).

- **Уточнение S6-37.** Согласованная правка CPL0 в `execute.rs` позволяет поточному кванту направлять член `pci_config` через общую шину платы. Обычный однопроцессорный LinuxDirect использует прежнюю ячейку CF8. [Проверка совместимости](STAGE_6_REPORT.md#s6-37--общая-конфигурация-pci).

**Статус.** S4-38 (доведение Linux до перехода к init) выполнялось итеративными
подсрезами серии S4-38L; S4-39 (userspace) — подсрезами S4-39a…S4-39j.
Родительский чекбокс S4-38 в плане не отмечен. Подсрез S4-39j принят после
ревью (`23197c7`, `4a8180a`; слияние `50a651a`). Подробности по подсрезам — в
[плане](../plans/STAGE_4.md) (S4-38L, декомпозиция и статусы S4-39) и в
[CPU_CONTRACT](../specs/CPU_CONTRACT.md) (разделы S4-38*, S4-39a…S4-39j);
здесь они не пересказываются.

**Что подтверждено (S4-39j).** Конфигурация: `od-runner linux-direct --run
--budget 3000000000 --set-cpl0 --cpuid-profile stage4a --user-mode` без
`--diagnostic-past-c7`, 128 MiB RAM, nonce `s4-38l4a`, cmdline
`console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1
od.nonce=s4-38l4a nokaslr`. Два холодных процесса, затем `od-runner
linux-direct-gate`: оба `PASS`, `counted: true`, `failures` пусты, run ID
`9d34785cdd85762428ba69873dd543700d87a50af5391e49d1728e79709b92c2`, отчёты
побайтно идентичны. Из отчёта: `cpl3_retired` 796, журнал syscall 9 записей
(83, 165, 257, 0, 3, 1, 39, 35, 60; `write` вернул 0x12 = длина строки
`OD_NONCE=s4-38l4a\n`), `halt_wakes` 2, `exit_kind` `budget_exhausted` (бюджет
исчерпан в цикле паники ядра после `exit(0)` PID 1), `tsc_hz` 1e9. Данные:
`docs/reports/data/stage4/s4-39j-gate-counted-cold1-stage4a-run.json`,
`…-cold2-…`, `s4-39j-gate-counted-verdict.json`. Ранние отчёты с флагом
диагностики (`PASS_DIAGNOSTIC_NOT_COUNTED`) остаются незачётными и не
пересчитывались.

**Отклонения от плана S4-39 и ограничения.**
- Конец прогона определяется строкой паники и журналом, а не `user_exit`; гейт
  принимает `budget_exhausted`, `exit_kind` и `cpuid_profile` не проверяет.
- Nonce фиксированный `s4-38l4a`; «новый nonce» плана не выполнен.
- На момент S4-46 IRQ из CPL3 не поддерживалось; implementation slice S4-39h
  добавлен 2026-11-15 и покрыт synthetic board test. Live timer/IRQ проверка
  гейтом по-прежнему не выполняется (в отчёте `halt_wakes` 2, но гейт его не
  требует).
- Профиль stage4a: без APIC и SSE/SSE2, 8259 PIC; JIT не использовался.
- Правило `counted` для stage4a — решение пользователя в день PASS; ревью этого
  решения не записано.
- vmlinuz и initramfs не хранятся в репозитории; подлинность ядра не
  подтверждена (S4-35).

Воспроизведение — `docs/testing/MACHINE.md`, раздел «Воспроизведение Linux
direct boot (S4-39j)».

## S4-40 — негативы гейта (с пробелами)

Добавлены негативные тесты `od-runner linux-direct-gate`
(`linux_gate::tests::s4_40_*`), код гейта не менялся; ложного `PASS` на
негативах нет. Коммиты `d494969`, `88631e6`, слияние `3d8ae31`;
`cargo test -p od-runner --bin od-runner`: 261 passed. Не покрыто по плану:
негативы при отключённой IRQ-маршрутизации, негатив запрещённого overlap,
повреждённый артефакт (проверка только по хешам). Гейт не проверяет
`exit_kind` и `cpuid_profile`. Родительский S4-40 в плане не принят.

## S4-41 — lifecycle в runner и VMHost (закрыт с ограничением)

S4-41a (headless, `od-runner linux-direct`): отказы конфигурации без запуска
гостя, отказы аргументов кодом 3, guest fault как отчётный останов, повторная
сессия, финальность STOP, замена паузы — продолжение после бюджетного останова
(`86b2df5`, `51c2085`, `874e9d4`; слияние `8f80ac1`; `od-runner --bin` 267
passed, `linux_direct_lifecycle_cli` 2 passed). S4-41b (VMHost,
`od-vmhost --linux-direct <config.json>`): подпроцесс `od-runner linux-direct`
через `posix_spawn`, pause/resume возвращают `unsupported`, невалидный конфиг
отклоняется до spawn, дескрипторы/зомби/временные каталоги на стороне VMHost
проверены (`8ec2071`, `31a5817`, `f997fa6`; слияние `ec9603e`; `swift test` 350
тестов). Ревью Opus high (read-only) обоих подсрезов: принято с замечаниями,
замечания исправлены. Закрыт по решению пользователя с ограничением.

**Не сделано (S4-41c, не выбран).** Через control v2 Linux-профиль
недостижим (CONFIGURE не несёт путь к ядру, принимает только профили 1 и 4);
pause/resume и STOP по control v2 для Linux-платы нет; живой статус и serial во
время прогона не выведены; доставка `od-runner` в бандле не сделана; реальный
прогон ядра через VMHost не запускался (скрипт-имитатор и реальный раннер на
фиктивных входах); утечки RAM и handles внутри раннера не проверялись.

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

| Величина | Регрессия по медианам | Разброс: 15 регрессий по повторам (медиана, min–max) | Производная из ячейки 16 | Точка доли 0 | S4-42a (формула) |
|---|---|---|---|---|---|
| Цена hit, нс | 3.21 | 3.20 (3.09–3.37) | 3.22 (ячейка 16 при цене промаха из ячейки 1) | не применимо | ≈3.3 |
| Цена промаха, нс | 35.48 | 35.49 (35.42–35.87) | не применимо | 35.48 (ячейка 1 обращение/страница, доля 0) | ≈33.7 |

«Производная из ячейки 16» и «точка доли 0» — не независимые проверки: промах
35.48 — это та же точка доли 0, что входит в регрессию, а 3.22 выводится из
ячейки 16 и этой же точки.

Главный довод в пользу линейной модели «стоимость = доля·hit + (1−доля)·промах» —
**максимальная невязка 0.03 нс** при 4 точках (R² 0.99999 приведён справочно).
Промах в cached-пути стоит на ≈5.2 нс больше uncached walk (35.5 против 30.3);
hit — в ≈9 раз дешевле walk (3.2 против 30.3), но это отношение полных
стоимостей обращения, включающих накладные расходы цикла и свёртки checksum
бенчмарка; отношение чистых цен больше, но не измерено.

**Совпадение с оценкой S4-42a.** Цена hit совпала (3.21 против ≈3.3, в пределах
разброса повторов). Цена промаха НЕ совпала с 33.7: получилось 35.5, то есть на
≈1.8 нс (≈5 %) больше, и это за пределами разброса повторов внутри процесса
(35.4–35.9). Сравниваемые ячейки не идентичны: в S4-42b ячейка с 4 обращениями
использует offsets 0/1024/2048/3072, в S4-42a — 0/8/64/4088; кроме того, сменился
бинарь (в `Scenario` добавлено поле `offsets`, всё перекомпилировано). Наблюдаемый
в этом прогоне общий сдвиг уровня около 5 % (uncached walk 30.3 против 29.0 в
S4-42a, +4.6 %; cached sequential/4 11.24 против 10.9; «промах минус uncached»
5.2 против 4.7 нс) не является измеренной величиной дрейфа. Возможные причины
сдвига промаха 33.7 → 35.5: межпроцессный дрейф, смена бинаря/раскладки кода,
sequential против random (S4-42a: 3 hit на 1024), набор offsets. Все четыре НЕ
проверены: сессия одна, процесс один. Причина не установлена; для разделения
нужны межпроцессные повторы трёх вариантов (бинарь S4-42a, новый бинарь с
`translation-micro`, новый бинарь с `translation-hit-share`) и ячейки random-1 и
hit-share-4 с offsets 0/8/64/4088 в одном процессе — это следующий подсрез. Вывод
для S4-42: оценка S4-42a для hit подтверждена производной оценкой (регрессия и
ячейка 16 при измеренном промахе), для промаха расходится на ≈5 % по причине,
которая не установлена.

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
фикстуры `3511e450…1ddc`, sha256 профиля запросов `fcd22115…5fc4`. Замер выполнен бинарём коммита `9acb857`; последующий коммит ревью добавил только assert-проверки порядка ячеек в `hit_share_fit` и сортировку в unit-тесте, числа и JSON не менялись, бенчмарк не перезапускался. Команда:
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

## S4-42c — межпроцессные повторы: причина сдвига цены промаха (S4-42 open)

Подсрез отвечает на «причина не установлена» из S4-42b: цена промаха трансляции
получилась ≈33.7 нс в S4-42a и 35.5 нс в S4-42b. Рамки те же (API трансляции
`od-machine`: uncached `probe_long64_4k` и capacity-one `TranslationProbeCache`;
Linux boot, JIT, IRQ/timer, Mac mini не входят). Ядро и код бенчмарка не менялись
(кода в этом подсрезе нет, только прогоны).

**Что измерено.** Три варианта, по 3 процесса каждого (`--process-run 1..3`, один
прогон на процесс), все 9 процессов подряд в одной сессии, последовательно (без
параллельных нагрузок от этого подсреза), чередуя варианты: A1 B1 C1 A2 B2 C2 A3
B3 C3.
- A: старый бинарь (commit `034b1f5`, merge S4-42a), `translation-micro`, полный
  набор S4-42a (45 ячеек), sha256 `cf335029…2367`.
- B: новый бинарь (commit `42ea9d8`), `translation-micro`, полный набор, sha256
  `df922776…fc37`.
- C: тот же бинарь, что в B, `translation-hit-share` (4 ячейки), тот же sha256.

Сокращений набора нет. Все 9 процессов приняты (correctness-проверки по oracle
как в S4-42a/b, хеш RAM фикстуры `3511e450…1ddc`). Между `034b1f5` и `42ea9d8`
менялся только `tools/bench` (`translation_micro.rs`, `main.rs`); код `od-machine`
идентичен. Значение процесса — медиана по его 15 внутренним повторам; ниже
«разброс» — (max−min)/median по трём процессам варианта.

**Бинари прошлых подсрезов и вариант A.** A — пересборка `034b1f5`, а НЕ бинарь,
которым мерили S4-42a (`6652218e…`, commit `9d96348`); S4-42b мерили бинарём
`9acb857` (sha256 `8b4611d6…`), а не `42ea9d8` (B/C). Поэтому сравнение A, B/C с
одиночными прогонами S4-42a/b смешивает сессию с пересборкой. Sha256 у A другой по
двум причинам: (1) `034b1f5` и `9d96348` отличаются перестановкой строки
`pub mod translation_micro;` в `tools/bench/src/lib.rs` и test-only assert в
`translation_micro.rs` (+9 строк в `mod tests`); (2) в бинарь при сборке зашиты
ревизия и sha256 исходников (`tools/bench/build.rs:51`), так что sha меняется при
любом коммите. Функционально измеряемый код тот же; совпадает ли раскладка кода
горячего пути — не проверено (проверить).

**Числа по процессам** (нс на обращение, медиана процесса: процесс 1 / 2 / 3).

| Величина | Вариант | Процессы 1 / 2 / 3 | Медиана по процессам | Разброс |
|---|---|---|---|---|
| uncached read-user/seq/256 (C: run-4, тот же walk) | A | 29.53 / 29.48 / 29.52 | 29.52 | 0.2 % |
| | B | 29.22 / 29.26 / 29.30 | 29.26 | 0.3 % |
| | C | 29.21 / 29.29 / 29.26 | 29.26 | 0.3 % |
| uncached run-1 | C | 29.24 / 29.24 / 29.34 | 29.24 | 0.4 % |
| uncached read-user/random/256 | A | 29.52 / 29.51 / 29.50 | 29.51 | 0.1 % |
| | B | 29.29 / 29.21 / 29.23 | 29.23 | 0.3 % |
| промах: cached random/256 (≈3 hit из 1024) | A | 34.36 / 34.30 / 34.32 | 34.32 | 0.2 % |
| | B | 34.06 / 34.05 / 34.09 | 34.06 | 0.1 % |
| промах: cached run-1 (доля hit 0) | C | 34.14 / 34.14 / 34.22 | 34.14 | 0.2 % |
| cached seq/256 (offsets 0/8/64/4088) | A | 11.05 / 11.02 / 11.04 | 11.04 | 0.3 % |
| | B | 10.96 / 10.98 / 11.01 | 10.98 | 0.5 % |
| cached run-4 (offsets 0/1024/2048/3072) | C | 10.97 / 10.99 / 11.00 | 10.99 | 0.3 % |
| промах − uncached (random/256 для A, B; run-1 для C) | A | 4.84 / 4.78 / 4.82 | 4.82 | 1.2 % |
| | B | 4.76 / 4.84 / 4.86 | 4.84 | 2.0 % |
| | C | 4.90 / 4.90 / 4.88 | 4.90 | 0.5 % |
| цена hit: формула S4-42a (A, B), регрессия (C) | A | 3.28 / 3.26 / 3.28 | 3.28 | 0.7 % |
| | B | 3.26 / 3.29 / 3.32 | 3.29 | 1.8 % |
| | C | 3.23 / 3.25 / 3.24 | 3.24 | 0.6 % |
| цена промаха: регрессия по 4 точкам | C | 34.14 / 34.15 / 34.22 | 34.15 | 0.2 % |

Для сравнения прошлые одиночные прогоны: S4-42a — uncached seq/256 29.0, промах
(cached random/256) 33.7, cached seq/256 10.9; S4-42b — uncached 30.31, промах 35.48,
cached run-4 11.24, цена hit 3.21. Свод по вариантам и значения по процессам (7 из
45 ячеек для A и B, 4 из 4 для C) —
[s4-42c-cross-process.json](data/stage4/s4-42c-cross-process.json). Полные
45-ячеечные отчёты (повторы, счётчики, checksum) всех 9 процессов —
[s4-42c-runs/](data/stage4/s4-42c-runs/) (`A1..A3.json` — вариант A, `B1..B3.json` —
B, `C1..C3.json` — C; ≈2.0 МБ), по ним воспроизводятся все числа таблицы и свода,
включая медиану uncached по всем 45 ячейкам (`_uncached_all_cells_median`, которой
нет в таблице).

**Что согласуется с данными.**
1. Межпроцессный разброс внутри сессии мал: 0.1–0.5 % для всех уровней (до
   1–2 % у производных величин, составленных из разностей). Сдвиг 33.7 → 35.5 нс
   (5 %) и сдвиг uncached 29.0 → 30.3 нс (4.6 %) в десятки раз больше. Значит,
   «разные процессы одной сессии» сами по себе сдвиг не объясняют.
2. Вывод (не прямой факт): уровень зависит от сессии и меняется в основном как
   целое, а не только на промахе. Между S4-42a и S4-42b напрямую: uncached +4.6 %
   (29.0 → 30.31), промах +5.3 % (33.7 → 35.48), то есть различие 0.67 п.п.;
   разность «промах − uncached» выросла 4.68 → 5.17 нс (+10 %): ≈0.5 нс из 1.78 нс
   сдвига промаха приходится на не пропорциональную часть. Значит, сдвиг двигался
   «в основном как целое», но не строго. В этой сессии новый бинарь `42ea9d8` (B, C)
   дал uncached 29.2–29.3 и промах 34.1, а не 30.3 и 35.5 из S4-42b (−3.5 %
   uncached, −3.8 % промах); бинарь S4-42b здесь не запускался (мерили `9acb857`,
   не `42ea9d8`), поэтому «уровни S4-42b не воспроизведены» смешано с различием
   бинарей. Вариант A относительно S4-42a: uncached +1.8 % (29.0 → 29.5), промах
   +1.8 % (33.7 → 34.3), но A — пересборка (см. выше), так что и этот сдвиг смешан
   с пересборкой. Источник сдвига между сессиями (частота, фон, размещение на ядрах,
   температура) НЕ установлен и не измерялся.
3. Смена бинаря между `034b1f5` и `42ea9d8` даёт малый сдвиг, не объясняющий 1.8 нс
   и притом в другую сторону: новый бинарь на всём быстрее старого (uncached 29.26
   против 29.52, −0.9 %; промах 34.06 против 34.32, −0.8 %; cached seq/256 10.98
   против 11.04) стабильно по всем трём чередующимся процессам, разность «промах −
   uncached» не меняется (4.82 против 4.84). В S4-42a→S4-42b «новый» был выше
   «старого» на 1.8 нс. Бинарь S4-42b (`8b4611d6…`, commit `9acb857`) здесь не
   пересобирался; от `42ea9d8` он отличается 14 добавленными строками (13 строк в
   `hit_share_fit`, включая комментарий, и 1 строка `sort_unstable` в `mod tests`),
   раскладка кода могла отличаться — не проверено.
4. Набор offsets не влияет: cached seq/256 с offsets 0/8/64/4088 (B) 10.98 против
   cached run-4 с offsets 0/1024/2048/3072 (C) 10.99 при одном бинаре.
5. Порядок (sequential против random) не влияет на цену промаха в пределах
   измерения: промах из random/256 (B) 34.06 (+≈0.09 за ≈3 hit из 1024 дают
   ≈34.15) против run-1 (C) 34.14 при одном бинаре; расхождение ≤0.1 нс против
   сдвига 1.8 нс в S4-42b.

**Чего не установлено.** Причина самого сдвига уровня между сессиями (S4-42a, S4-42b
и этой): данных о частоте/температуре/фоновой нагрузке нет; машина общая с другими
сессиями, load average в начале процессов 2.4–5.3, корреляции с результатами не
проверялись (внутри этой сессии результаты стабильны при таком load). Вклад
пересборки бинаря S4-42b не отделён от сессии. Поэтому ответ неоднозначен:
наиболее согласованное с данными объяснение — сдвиг уровня между сессиями (не
процесс, не offsets, не порядок, не смена бинаря между `034b1f5` и `42ea9d8`), но
это вывод методом исключения по трём сессиям, а не прямое измерение причины.

**Не сделано.** Ячейка sequential с 4 обращениями на страницу и offsets
0/8/64/4088 в серии hit-share не добавлена: серия hit-share зафиксирована
константами `RUN_1/2/4/16`, параметр `Scenario::offsets` есть, но 5-я ячейка
потребовала бы правки серии и fit; по условию задачи код не менялся. Эффект
offsets при этом проверен косвенно (п. 4) тем же бинарём в `translation-micro`.

**Для S4-42.** Цена промаха capacity-one cache не имеет одного стабильного
значения: 33.7 / 35.5 / 34.1–34.3 нс по трём сессиям (±≈3 %), цена hit 3.2–3.3
нс стабильна (разброс сессий мал на фоне). Выводы S4-42b про зависимость от доли
hit и безубыточность (≈16 %) от сдвига не зависят качественно, но числа следует
читать как «±3 % между сессиями».

**Среда.** Apple M3 Max, macOS 26.7.1, aarch64-apple-darwin, `rustc 1.99.0`,
профиль `release` (без LTO), питание от AC, один сеанс ≈16 мин (20:44–21:00).
Команды: `target/release/od-bench translation-micro|translation-hit-share
--process-run N --output <path>`. Проверки: `cargo check --workspace --all-targets`,
`cargo fmt --all -- --check` (известные расхождения `storage_abi_v1.rs:139` и
`tools/image-tool/tests/cli.rs:109`).

**Ограничения.** Ноутбук (M3 Max), не Mac mini; одна сессия из трёх процессов на
вариант, сравнение с S4-42a/b — по их одиночным прогонам из другого времени;
без изоляции от фоновой нагрузки; вариант A — пересборка, а не бинарь S4-42a, и
бинарь S4-42b не запускался; цена hit/промаха включает накладные расходы
цикла и checksum бенчмарка. S4-42 остаётся открытым: не сделаны 2 MiB страницы,
tracked A/D-путь, JIT cold/warm, ограниченный Linux boot, IRQ/timer, Mac mini.

## S4-43a — reset / churn / ограниченные ресурсы для трансляции и кэша (S4-43 open)

Узкий подсрез S4-43: только API трансляции `od-machine`
(`MutableTranslationProbeSession`, `TranslationProbeCache`) и `PhysicalMemory`.
Linux boot, IRQ/timer, event queue, executable (JIT) cache, SMP в него не входят.
Ядро не менялось; добавлен один тест
`tools/bench/tests/s4_43a_translation_churn.rs` (запуск:
`cargo test -p od-bench --test s4_43a_translation_churn`, профиль `test`/debug;
`S4_43A_JSON_OUT=<path>` пишет сводку). Данные:
`docs/reports/data/stage4/s4-43a-churn.json`.

**Что проверяется.** Каждый ответ кэша сверяется с независимым cache-free обходом
(`oracle` читает не RAM, а обычную Rust-модель записанных слов таблиц; после
каждого сегмента модель сверяется с реальной RAM). 2000 итераций churn; в каждой:
прогрев 5 комбинаций (read/write ring3, read/write ring0, fetch ring0), запись PTE
без сброса, проверка, что резидентный hit до инвалидации (если есть) равен ровно
старому ответу (hit считается stale, только если ответ oracle после записи PTE
отличается), затем инвалидация (чётные итерации INVLPG, нечётные MOV CR3 с тем же
корнем), затем сверка всех комбинаций с oracle (реальная проверка сброса кэша это
только первый lookup после инвалидации; остальные Walk следуют из кэша на одну
запись и разных ключей и отдельной проверкой не являются; повтор транслируемого
ключа Hit; фаулты не кэшируются) и ещё одной случайной страницы. Действия по
кругу: unmap, повторный map (новый кадр), remap (новый кадр), write→read-only, user→supervisor, NX. Эпохи чередуют
CR0.WP=1/0 (`_wp` варианты исполнения).
- Code writes: каждые 10 итераций запись нулей в байты INVLPG кода (нет retire,
  кэш не сброшен), восстановление байтов (retire, кэш сброшен), remap страницы
  кода на другой кадр при резидентной fetch-трансляции старого кадра (fetch обязан
  идти новым кадром). Оговорка: `SessionFetchMemory` читает RAM без кэша байтов
  кода, поэтому «fetch видит новые байты» верно по построению. Ценность шагов 1-2 в
  дифференциальности (нули дают отсутствие retire, восстановление дает retire) и в
  проверке, что неретировавшийся INVLPG не сбрасывает кэш; значимый шаг здесь
  remap (кэш fetch-трансляции не должен подменять новую страницу).
  Генерация страницы `PhysicalMemory` = 1 + число committed writes для каждой
  записанной страницы (проверка в конце каждой эпохи).
- Reset: новая сессия каждые 25 итераций (пустой кэш, первый probe каждого
  ключа = Walk, значения = oracle); полный reset каждые 100 итераций (новая RAM,
  генерации = 1, начальные трансляции всех 18 страниц × 5 комбинаций совпадают с
  baseline первой эпохи); после каждой эпохи `TranslationProbeCache` над
  неизменяемым снимком: probe = oracle, `clear()` → `is_empty()`, следующий
  probe = Walk.
- Watchdog: вся нагрузка в рабочем потоке, ожидание `recv_timeout`; самопроверка
  watchdog (зависшее замыкание за 200 мс объявляется TimedOut); бюджет эпохи.

**Лимиты (константы теста).** 2000 итераций основного прогона (+100 прогрев,
+200 baseline); 16 churn-страниц + страница кода + sentinel; RAM 1 MiB на эпоху;
пул кадров 64; не более 8 committed writes на итерацию (trace записей ограничен);
watchdog 120 с, бюджет эпохи 20 с; рост RSS < 5 MiB; живая куча после 2000
циклов равна куче после baseline в пределах 64 KiB; пик кучи не выше 2 RAM +
0.5 MiB; доля итераций с по-настоящему stale hit (резидентный hit, ответ которого
отличается от oracle после записи PTE) не ниже 20 % (защита от пустой нагрузки, а
не доказательство чувствительности).

**Результат (один прогон, macOS aarch64, debug).**

| Проверка | Значение |
|---|---|
| итерации основного прогона / эпохи / сессии | 2000 / 20 / 80 |
| сравнений с oracle | 33 200 (основной), 3 320 (baseline) |
| INVLPG / MOV CR3 инвалидаций | 1000 / 1000 |
| резидентных hit до инвалидации (`resident_hits`) | 1479 из 2000 (74 %) |
| из них по-настоящему stale (`stale_hits`, ответ отличается от oracle) | 963 из 2000 (48 %) |
| действия (unmap/map/remap/ro/supervisor/nx) | 334/334/333/333/333/333 |
| code: garbage write / rewrite / remap | 200 / 200 / 200 |
| committed writes (trace), максимум за итерацию | 3860, 8 |
| несовпадений с oracle | 0 |
| живая куча (прогрев, baseline, основной) | 15 678 / 15 678 / 15 678 байт, разница 0 |
| пик кучи над исходной | 1 053 752 байта (≈ одна RAM 1 MiB) |
| RSS после прогрева, после основного | 4 685 824 и 4 931 584 байта, +245 760 (лимит 5 MiB) |
| время основного прогона / всего теста | 102 мс / 129 мс (+200 мс самопроверка watchdog) |

**Контрольная проверка на сломанном кэше (сделана; правка исходника временная,
откатана, не в коммите).** (1) отключение очистки при INVLPG: тест падает,
`invalidation must empty the cache`; (2) тег кэша без access/context: тест
падает, `first probe of page 0 in a new session hit`. Оба прогона сделаны на коде
теста до ревью (`f55e158`) и поймали структурные утверждения раньше сверки
значений. (3) Контроль значений (на `2397ed9`): в тесте не обновляется
`mirror` для действия Remap, кэш и RAM корректны, oracle по устаревшей модели.
Тест падает именно на сверке значений: `post-invalidation target: Read/Ring3 at
0x40c018: cache 0x3f018 != oracle 0x1c018`; структурные утверждения до этого
места проходили. Значит сверка значений с oracle реально ловит рассогласование.

**Не проверено / ограничения.** Linux boot, повторные STOP/boot в runner,
IRQ/timer reprogramming и старые IRQ/handles после reset, event queue и pending
events, executable (JIT) cache и его лимит, trace runner, SMP, 2 MiB страницы,
tracked A/D. Запись кода идёт через `write_pte` (выровненные 8-байтные записи), а
не через guest store CPU. «Reset» здесь означает пересоздание сессии/RAM и
`TranslationProbeCache::clear`; у мутируемой сессии собственного `clear` нет.
Прогон занимает около 0.1 с в debug: это ограниченный детерминированный stress
(2000 циклов), а не долговременная надёжность, не SMP-безопасность и не
готовность Linux этапа 5. RSS измерен через `ps` в одном процессе, один прогон;
куча — счётчик глобального аллокатора теста; рабочий поток watchdog может
попасть в окно измерения (сотни байт при допуске 64 KiB), не исправлялось.
Высокие биты физического адреса не проверялись: все кадры ниже 1 MiB. Поля
`test`, `source_commit`, `result`, `control_checks` и `manual_fields` в JSON
добавлены вручную (перечислены в `manual_fields`), остальное пишет тест.
S4-43 остаётся открытым.

## S4-43b — ограниченные очереди событий и trace под churn/reset (S4-43 open)

Узкий подсрез S4-43: очереди и trace слоя `od-machine`, которые доступны через
публичный API без Linux. Ядро не менялось; добавлен один тест
`tools/bench/tests/s4_43b_bounded_queues.rs` (запуск:
`cargo test -p od-bench --test s4_43b_bounded_queues`, профиль `test`/debug;
`S4_43B_JSON_OUT=<path>` пишет сводку). Данные:
`docs/reports/data/stage4/s4-43b-queues.json`. Ограниченный stress, а не
долговременная надёжность.

**Очереди и их лимиты (найдено в коде).** Перечень получен grep по `od-machine`
(`VecDeque`, `EventQueue::new`, константы `*_CAPACITY`/`*_LIMIT`, `pop_front`,
`truncate`) и не является полным по всем крейтам.

| Очередь | Ёмкость | Переполнение | Проверено |
|---|---|---|---|
| `EventQueue` (`event_queue.rs`) | параметр `new(capacity)`, Vec зарезервирован | ошибка `EventQueueError::Full`, очередь не меняется | да (cap 8) |
| `UartRxEventPump`: события + список payload (`uart_rx_events.rs`) | параметр `new(capacity)` | `Full` из `schedule`, payload не добавляется | да (cap 8) |
| IRQ-очередь `InterruptPump` | `INTERRUPT_PUMP_QUEUE_CAPACITY` = 16 | `PumpQueueError::Queue(Full)`; due-событие при уже поднятом IRR сливается и считается в `coalesced_irqs` | да |
| trace `InterruptPump`, режим `Bounded` (по умолчанию) | `INTERRUPT_PUMP_TRACE_LIMIT` = 512 | явный отказ: `Rejected(TraceLimit)`, прогон финальный | да |
| trace `InterruptPump`, режим `Ring(n)` | параметр `n` (без верхнего предела) | вытеснение старейшего, счётчик `trace_dropped`, `seq` непрерывен | да (n = 64) |
| очередь событий дисплея (`display_device.rs`) | `DISPLAY_EVENT_CAPACITY` = 16 | `DisplayRequestError::QueueFull` | частично: через публичный API недостижимо |
| `SyscallJournal.entries` (`linux_direct.rs`) | `SYSCALL_JOURNAL_CAPACITY` = 32 записей; `entered`/`returned` — u64-итоги без предела | перезапись: `pop_front` старейшей записи при `Enter`, итоги продолжают расти; возврат вызова, уже вытесненного из журнала, не сопоставляется | нет (только unit-тест в крейте) |
| очередь моста `PitIrq0Owner` (`stage4_pit_irq0_probe.rs`) | `BRIDGE_QUEUE_CAPACITY` = 16; `deliver_due` обрабатывает не более 16 фронтов за вызов | ошибка очереди возвращается как `Err(String)`; цикл ограничен константой | нет |
| таймерная очередь `apic_pair.rs` | 1 (`EventQueue::new(1)`) | одна ожидающая отметка; перепрограммирование заменяет её через `replace_single`, роста нет | нет |
| очереди источников PIT/PIC и PIT/IOAPIC | 4 (приватная константа) | ошибка счётчика `queue_faults`, источник перестаёт планировать | нет |
| `ErrorLog` NVMe (`nvme/admin.rs`) | `ERROR_LOG_ENTRIES` = 64 (общий счётчик `count` — u64) | `push_front` и `truncate`: старейшая запись отбрасывается, счётчик растёт | нет |
| `AerState` NVMe (`nvme/admin.rs`) | `AER_LIMIT` = 4 на outstanding+ready; ожидающих событий не больше одного на тип (маска, 8 типов) | запрос сверх предела завершается `AER_LIMIT_EXCEEDED`; повторное событие того же типа маскируется | нет |
| `TranslationProbeCache` | одна запись | замена | S4-43a |
| NVMe SQ/CQ | 2..=1024, задаёт гость | неверный размер отвергается admin-командой | нет |

Неограниченных очередей в перечне не найдено; перечень — по grep по `od-machine`,
не полный по всем крейтам. У каждой очереди из перечня есть явный предел и либо
явный сигнал (ошибка, отказ, счётчик), либо документированная перезапись
(`SyscallJournal`, `ErrorLog`, `Ring`-trace с `trace_dropped`).
Находки (не блокеры): (1) очередь дисплея недостижима для заполнения: непринятые
`MODE_REQUEST` экрана 0 сливаются, наблюдаемый максимум 1 из 16; ветка `QueueFull`
покрыта только unit-тестом в самом крейте. (2) У trace `InterruptPump` нет
`clear`: сбросить его можно только пересозданием pump; `trace_next_seq` и
`coalesced_irqs` накопительные по задумке, `reset_irq_queue` очищает только
IRQ-очередь. (3) `Ring(n)` не ограничивает `n` сверху, фиксированный предел есть
только у `Bounded`. (4) Переполнение `Bounded` завершает pump окончательно, а
ожидающие IRQ-события остаются в очереди до `reset_irq_queue`; сколько именно
событий осталось при отказе (в прогоне 9), тест не моделирует и проверяет только
границы `0 < len <= 16`. (5) `SyscallJournal` и `ErrorLog` при переполнении
перезаписывают старейшие записи без ошибки; сигнал потери есть только в виде
итоговых счётчиков.

**Что проверяется.** Каждый ответ очереди сверяется с моделью на обычных Rust-
структурах (порядок по (deadline, вставка), Full/DuplicateKey/NotFound, длина,
ближайший deadline, порядок и значения доставленных байт UART, число входов в
обработчик по различным моментам due, `coalesced_irqs`, время после idle advance).
Проверяется: длина каждой очереди не превышает объявленную ёмкость после каждой
операции; на каждой попытке переполнения проверяется явный `Err(Full)` (assert
на каждой операции, счётчики попыток и отказов только отражают число таких
проверок); после reset очередь пуста, токен старой генерации даёт
`StaleGeneration`, а события, ждавшие в момент reset, не доставляются.
Trace `Ring`: число записанных событий считается на стороне теста независимо
от pump (на доставленное прерывание pump записывает ровно 5 событий:
`irr`, `entry`, `eoi`, `iretq`, `hlt`, на слитое due-событие одно `coalesced`,
загрузка до первого HLT даёт `sti`, `hlt`); после каждой итерации
`trace_next_seq` равен счёту теста, длина равна `min(cap, записано)`,
`trace_dropped` равен `записано - длина`, `seq` непрерывен и заканчивается на
`записано - 1`. Это согласованность учёта; вытеснение проверено условиями
`len <= cap` и непрерывностью `seq`. STOP при ожидающих событиях: немедленный
`Stopped`, события не теряются (длина очереди 3 проверяется и после первого, и
после повторного `run`), повторный `run` снова `Stopped`, нет зависания. Каждый
прогон pump идёт с бюджетом шагов, а весь тест в рабочем потоке под watchdog
(самопроверка: зависшее замыкание за 200 мс объявляется TimedOut). Рост кучи:
сразу после создания объектов (0 итераций) снимается удерживаемый объём, он
сравнивается с объёмом после прогона (прогрев 100, baseline 200, основной 2000
итераций) и между прогонами. Список payload UART приватен, поэтому лишний payload
после `Full` виден только через кучу: список зарезервирован под ровно `capacity`
элементов, и лишний элемент вызывает реаллокацию.

**Лимиты (константы теста, сужены до измеренного с запасом).** 2000 итераций
основного прогона на очередь (+100 прогрев, +200 baseline); ёмкости EventQueue и
UART 8; `Ring(64)`; до 20 IRQ-событий за итерацию при ёмкости 16; watchdog 120 с,
бюджет сценария 30 с; рост RSS < 5 MiB; разница удерживаемой кучи 2000 vs 200
итераций не более 1 KiB (измерено 0); остаток после drop не более 512 байт
(измерено 0 и 280; 280 байт — один лист `BTreeMap` в `Counters.extra`, который
ещё держит возвращаемая сводка, а не утечка очереди; комментарий есть в тесте);
рост кучи после создания: EventQueue ≤ 64 (изм. 0), UART ≤ 96 (изм. 64), pump
≤ 12 KiB (изм. 9 496, ring-вектор trace), дисплей ≤ 256 (изм. 0); пик над началом
сценария: EventQueue ≤ 1 KiB (изм. 384), UART ≤ 2 KiB (изм. 968), pump ≤ 80 KiB
(изм. 57 101), дисплей ≤ фреймбуфер 36 MiB + 192 KiB (изм. 165 200 сверх
фреймбуфера: 128 KiB RAM шины и состояние устройства; лимит сужен с «фреймбуфер +
256 KiB»). Фреймбуфер выделяется конструктором шины и входит в удерживаемую кучу
дисплея одинаково при любом числе итераций.

**Результат (один прогон, macOS aarch64, debug).**

| Очередь | итераций | макс. длина / ёмкость | попыток переполнения, на каждой проверен `Err(Full)` | reset (очередь пуста) | сравнений с моделью |
|---|---|---|---|---|---|
| EventQueue | 2000 | 8 / 8 | 272 | 40 (40) | 8 000 |
| UartRxEventPump | 2000 | 8 / 8 | 921 | 80 (80) | 20 833 |
| IRQ-очередь pump | 2000 | 16 / 16 | 910 | 80 (80) | 27 792 |
| очередь дисплея | 2000 | 1 / 16 | 0 (недостижимо) | 60 (60) | 6 000 |

| Проверка | Значение |
|---|---|
| ring trace pump: записано / вытеснено / макс. длина | 49 917 / 49 853 / 64 из 64; счёт тестом совпал после каждой из 2000 итераций |
| входов в обработчик / принято IRQ-событий / отменено / сброшено reset | 8 073 / 17 623 / 483 / 776 |
| байт UART доставлено / отброшено reset | 9 132 / 396 |
| отказов `NoDriver` дисплея (отдельный счётчик, не stale token) | 60 |
| `Bounded` trace: итераций до предела, длина, dropped | 7, 512, 0; `Rejected(TraceLimit)`, повторный `run` тот же результат; при отказе в очереди осталось 9 событий (границы 0 < len ≤ 16), после `reset_irq_queue` 0 |
| `Ring(0)` / смена режима после записи | отвергнуты (`ZeroCapacity`, `Recorded`) |
| удерживаемая куча сразу после создания → после прогона (EventQueue, UART, pump) | 384 → 384, 904 → 968, 46 531 → 56 027 байт; при 100, 200 и 2000 итерациях значения одинаковы |
| разница 2000 vs baseline, все очереди | 0 байт (лимит 1 KiB) |
| остаток после drop | 0 (EventQueue), 280 байт у остальных (лист `BTreeMap` сводки) |
| пик кучи над исходной (EventQueue, UART, pump) | 384, 968, 57 101 байт |
| RSS: рост между прогревом и концом основных | +131 072 байта (лимит 5 MiB) |
| время всего теста | 0.97 с (включая самопроверку watchdog 200 мс) |

**Контрольная поломка (сделана; правки исходника временные, откатаны
`git checkout`, не в коммите, `git status` чистый).** (1) `EventQueue::reset` не
вызывает `entries.clear()`: тест падает, `queue after reset`. (2) проверка
ёмкости в `schedule` `==` заменена на `>` (очередь растёт сверх ёмкости):
`schedule key 11`, ожидался `Err(Full)`, получен `Ok(())`. (3) Ring trace не
увеличивает `trace_dropped`: `every recorded event is retained or counted as
dropped: left 64 right 77` (на первой версии теста; в нынешней проверка идёт
против счёта теста). (4) `UartRxEventPump::schedule` добавляет payload и при
`Full`: тест падает на `delivery order: left Some(170) right Some(90)`, потому
что после reset ключ 1 переиспользуется и находит устаревший payload; проверка
кучи при этом ещё не достигнута. (5) то же, но лишний payload с ключом `!key`
(не переиспользуется): тест падает на проверке кучи, `heap grew by 448 bytes
after creation (limit 96)`. Значит и функциональная проверка, и проверка кучи по
отдельности ловят лишний payload после `Full`. Все поломки ловятся
структурными утверждениями, не таймаутом.

**Не проверено / ограничения.** Linux boot, IRQ и timer на реальном ядре (pump
гоняет диагностический образ с одним вектором 0x62 и локальным APIC без
guest-программирования таймера), executable (JIT) cache и его лимит, SMP,
долговременный прогон, trace runner и журналы вне `od-machine`, очереди PIT
(приватный предел 4), мост `PitIrq0Owner`, таймерная очередь `apic_pair`,
`SyscallJournal`, `ErrorLog` и `AerState` NVMe, NVMe SQ/CQ. Очередь дисплея
заполнить через публичный API нельзя, поэтому её переполнение здесь не
проверено. Точное число событий, оставшихся в очереди при отказе `Bounded`, не
моделируется. Один прогон, RSS измерен через `ps`, куча — счётчик глобального
аллокатора теста; перенос на другой хост/профиль (release) не проверялся. Поля
`test`, `source_commit`, `result`, `control_checks`, `findings`,
`inventory_added_by_review` и `manual_fields` в JSON добавлены вручную
(перечислены в `manual_fields`), остальное пишет тест. S4-43 остаётся открытым.

## S4-43c — повторные прогоны в отдельных процессах

Подсрез S4-43c: код не менялся. Тест-бинари S4-43a и S4-43b собраны один раз
(`cargo build -p od-bench --tests`, debug, коммит `208bbf4`) и запущены напрямую по
3 раза каждый, каждый запуск в отдельном процессе, с `--nocapture` и
`S4_43A_JSON_OUT` / `S4_43B_JSON_OUT`. Время `wall` измерено снаружи. Сводка:
`docs/reports/data/stage4/s4-43c-repeats.json`, сырые JSON процессов:
`docs/reports/data/stage4/s4-43c-runs/` (6 файлов, около 60 KiB).
sha256 тест-бинарей: S4-43a `7b299da5e9c9f22ac1d34b87d86b8bf49ee90435eac07315c124cb57d58c76e5`,
S4-43b `16de7f138633fcbe8696e8e93aead83d03fd1174491de3b61a63f6a8a18d817d`.

| Тест | Процесс | passed | wall, с | Ключевые числа |
|---|---|---|---|---|
| 43a | 1 | да | 0.383 | resident_hits 1479, stale_hits 963 (48.1 %), сравнений 33 200 + 3 320, куча main − baseline 0, RSS main − warmup +1 310 720 |
| 43a | 2 | да | 0.385 | те же; RSS main − warmup +1 310 720 |
| 43a | 3 | да | 0.373 | те же; RSS main − warmup +1 310 720 |
| 43b | 1 | да | 0.999 | длины 8/8, 8/8, 16/16, 1/16; попыток переполнения 272/921/910/0; ring trace 49 917 записано, 49 853 вытеснено, длина 64; куча main − baseline 0; RSS +131 072 |
| 43b | 2 | да | 1.001 | те же; RSS +131 072 |
| 43b | 3 | да | 0.955 | те же; RSS +131 072 |

**Разброс.** Значения всех счётчиков, длин очередей, куч и числа попыток
переполнения совпали во всех трёх процессах (сравнены все плоские поля JSON
процессов; файлы при этом не побайтно равны: 43b p3 16 457 байт против 16 458 у
p1/p2, из-за длины чисел времени). Различаются только время и абсолютный RSS:
wall 43a 0.373-0.385 с (медиана 0.383), 43b 0.955-1.001 с (медиана 0.999); RSS
после main 43a 6 029 312-6 045 696 байт, 43b 42 778 624-42 827 776 байт (разброс
до 48 KiB). Рост RSS одинаков во всех процессах: 43a +1 310 720 (лимит 5 MiB),
43b +131 072.

**Подтверждено во всех 6 процессах.** 43a: 0 расхождений с oracle (любое
расхождение роняет тест), живая куча после 2000 циклов равна куче после baseline
(разница 0), доля по-настоящему stale hit 48.1 % при минимуме 20 %, не более 8
committed writes за итерацию, самопроверка watchdog сработала. 43b: максимальная
длина каждой очереди не больше её ёмкости, число попыток переполнения равно числу
явных отказов `Err(Full)`, число reset равно числу проверок пустоты очереди после
reset, удержанная куча main равна baseline (0), `Bounded` trace отказывает на 512
событиях за 7 итераций (`Rejected(TraceLimit)`), при отказе в очереди 9 событий
(в границах 0 < len ≤ 16), после `reset_irq_queue` 0.

**Отличия от одиночных прогонов S4-43a/b.** Счётчики те же, что в
`s4-43a-churn.json` и `s4-43b-queues.json`. Отличаются абсолютные RSS (другой
момент и способ запуска: бинарь напрямую, а не `cargo test`) и у 43a живая куча:
до прогрева 14 913 против 14 958 байт, после фаз 15 633 против 15 678; разница 45
байт в обоих случаях, вероятно из-за длины пути в окружении (не проверялось).
Рост RSS 43a между прогревом и концом main здесь 1 310 720 байт против 245 760 в
одиночном прогоне. Рост приходится на фазу baseline (+1 114 112 байт, в одиночном
прогоне +49 152), main добавляет +196 608, как и раньше. Причина не установлена;
гипотеза: вероятно, первое касание страниц RAM 1 MiB в другом порядке
выделения, проверка не выполнялась. Против гипотезы: в одиночном прогоне warmup
тоже выделял RAM 1 MiB (+1 982 464 байта), а baseline дал только +49 152. Лимит
5 MiB не нарушен; число «+245 760» в разделе S4-43a — значение одного запуска и
не воспроизводится. Данные раздела S4-43a не перезаписаны.

**Ограничения S4-43c.** 3 процесса на тест, один хост (macOS aarch64), один
коммит, профиль debug, без изоляции от фоновой нагрузки; недетерминизм не наблюдался в
3 процессах на одном хосте; статистики по хвостам это не даёт.
Сводка `s4-43c-repeats.json` собрана одноразовым python-скриптом вне
тестов; скрипт не закоммичен (сырые JSON процессов в репозитории). Что скопировано
из сырых JSON, что взято из stdout, что измерено снаружи и что написано вручную,
перечислено в `manual_fields` сводки; сырые JSON процессов пишет сам тест.

## S4-43 — закрытие

**Статус.** S4-43 закрыт как ограниченная проверка трансляций, кэша и очередей;
часть планового состава S4-43 не выполнена и передана (см. «Передача остатков»).

**(а) Что проверено.**
- S4-43a ([раздел](#s4-43a--reset--churn--ограниченные-ресурсы-для-трансляции-и-кэша-s4-43-open)):
  `MutableTranslationProbeSession`, `TranslationProbeCache` и `PhysicalMemory` под
  2000 итерациями churn с reset: 33 200 сверок с независимым oracle, 0
  расхождений, 1000 INVLPG + 1000 MOV CR3, 963 по-настоящему stale резидентных hit
  (48 %), живая куча не растёт (0 байт). RSS +245 760 из S4-43a — значение одного запуска;
  в S4-43c +1 310 720; утверждается только «ниже 5 MiB».
- S4-43b ([раздел](#s4-43b--ограниченные-очереди-событий-и-trace-под-churnreset-s4-43-open)):
  `EventQueue`, `UartRxEventPump`, IRQ-очередь и trace `InterruptPump` (режимы
  `Bounded` и `Ring`), очередь дисплея (частично) под 2000 итерациями churn с
  reset: длины в пределах ёмкости, на каждой попытке переполнения явный
  `Err(Full)`, после reset очередь пуста и токены старой генерации отвергаются,
  удержанная куча не растёт, `STOP` при ожидающих событиях не теряет их и не
  зависает.
- S4-43c (раздел выше): 3 отдельных процесса на каждый тест, все 6 прошли, все
  счётчики и инварианты совпали, различаются только время и RSS.
- Контрольные поломки. S4-43a - три: (1) отключена очистка кэша при INVLPG и
  (2) тег кэша без access/context, оба прогнаны на коде теста до ревью (`f55e158`),
  падение на структурных утверждениях; (3) не обновлён `mirror` для remap (на
  `2397ed9`), падение на сверке значений с oracle (по разделу S4-43a). S4-43b -
  пять: `EventQueue::reset` без `clear`; проверка ёмкости `>` вместо `==`; ring
  trace без `trace_dropped` (прогнано на первой версии теста, нынешняя проверка
  идёт против счёта теста); лишний payload UART при `Full` в двух вариантах
  (функциональная проверка и проверка кучи). Ни одна не поймана таймаутом. Это
  контроль чувствительности тестов, а не доказательство отсутствия ошибок в ядре.

**(б) Что НЕ проверено и почему это остаётся.**
- Linux boot и повторные STOP/boot в runner: тесты работают с диагностическими
  образами и API `od-machine`, не с загрузкой Linux.
- IRQ/timer reprogramming на реальном ядре и старые IRQ/handles после reset:
  pump гоняет образ с одним вектором 0x62, без guest-программирования таймера.
- JIT / executable cache и его лимит: он не входил в проверенные API, отдельный срез
  S4-43 не делался.
- SMP: все тесты однопоточные по модели (рабочий поток нужен только watchdog).
- Страницы 2 MiB и tracked A/D путь: не входили в нагрузку.
- Long-run: 2000 итераций, секунды на тест; это не долговременная надёжность.
- Trace runner и журналы вне `od-machine` (`SyscallJournal`, мост `PitIrq0Owner`,
  таймерная очередь `apic_pair`, очереди PIT/PIC и PIT/IOAPIC): проверены только в
  инвентаре S4-43b, без нагрузки.
- NVMe SQ/CQ, `ErrorLog` и `AerState`: в инвентаре, без нагрузки.
- Очередь дисплея: заполнить через публичный API нельзя, переполнение не
  проверено.
- Release-профиль и другие хосты: только debug, macOS aarch64; RSS и куча зависят
  от аллокатора и запуска (см. разницу 43a выше).
- Высокие биты физического адреса (все кадры ниже 1 MiB) и запись кода через
  guest store CPU (запись PTE идёт через `write_pte`).

**(в) Вывод.** Ограниченная проверка трансляций, кэша и очередей под churn/reset
пройдена; долговременная надёжность и Linux-поведение S4-43 не подтверждают.

**Передача остатков.**
- (а) Boot/STOP/reset в runner и `--reboot-cycles` (`docs/plans/STAGE_7.md`,
  S7-26) → S4-44 (воспроизводимый набор) и далее STAGE_7.
- (б) Guest-программирование таймера (timer reprogramming) и старые IRQ/handles
  после reset → остаток S4-42 или S4-44; владелец назначается при планировании
  S4-44.
- (в) Лимит executable cache (`crates/od-jit/src/dispatch.rs`) → S4-42 (JIT
  cold/warm) или S4-44.
- (г) Stress mode runner в `tests/system/` (сейчас тесты S4-43 лежат в
  `tools/bench`) → S4-44.
- (д) Явный лимит guest RAM: сейчас проверены только host heap/RSS, RAM
  фиксирована 1 MiB → S4-44.
- (е) Остаточные ограничения, не закрываемые в этапе 4 (SMP, long-run, release и
  другие хосты, 2 MiB, tracked A/D, NVMe SQ/CQ, `ErrorLog`/`AerState`, очереди
  вне `od-machine`) → `docs/KNOWN_ISSUES.md` (S4-46).
- Пункты, не относящиеся к S4-44 напрямую: (б) и (в) принадлежат S4-42 (остаток),
  (е) принадлежит S4-46; пункт (а) после S4-44 уходит в STAGE_7.

**(г) Ссылки.** Подразделы: S4-43a, S4-43b, S4-43c (выше). Данные:
`docs/reports/data/stage4/s4-43a-churn.json`,
`docs/reports/data/stage4/s4-43b-queues.json`,
`docs/reports/data/stage4/s4-43c-repeats.json`,
`docs/reports/data/stage4/s4-43c-runs/`.

## S4-44 — результат локального прогона

**Статус.** S4-44a (локальный набор) выполнен один раз; результат `failed` (2 шага из 9 с ненулевым кодом, из них один — зависимость от состояния `work/`, один — реальное падение теста). S4-44 в целом открыт: Linux boot, Mac mini и сборка guest/firmware artifacts не выполнялись. Скрипт: `tools/stage4/run_local_set.sh` (sha256 `77d22562d08ce1b9bf6da069f909f54372268db9cb041a2b5a57069445aa7b09`), сводка: `docs/reports/data/stage4/s4-44a-local-set.json`, порядок запуска — `docs/testing/MACHINE.md`, раздел «S4-44 — воспроизводимый локальный набор этапа 4».

**Окружение.** commit `47dd472` (дерево чистое, кроме нового `tools/stage4/`), ветка worktree от main, rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0, macOS 26.7.1, arm64, 2026-11-14. Общая длительность 788 с; шаг-таймаут 1200 с, ни один шаг не исключён.

| шаг | статус | код | длительность, с | log sha256 |
| --- | --- | --- | --- | --- |
| `fmt` | known_divergence | 1 | 4 | `cb81c490…` |
| `check-workspace` | passed | 0 | 15 | `8f7c4d73…` |
| `test-od-isa` | passed | 0 | 39 | `77188671…` |
| `test-od-machine` | passed | 0 | 83 | `64717430…` |
| `test-od-jit` | passed | 0 | 6 | `b1529ead…` |
| `test-od-host` | failed | 101 | 128 | `87765124…` |
| `test-od-runner` | failed | 101 | 454 | см. JSON |
| `test-od-bench` | passed | 0 | 19 | см. JSON |
| `test-rest-of-workspace` | passed | 0 | 40 | см. JSON |

**Итог:** 6 passed (включая `check-workspace`), 1 known_divergence (`fmt`: только `crates/od-host/src/storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs`), 2 failed, 0 timeout, 0 исключено. `overall_local` = `failed`. Полные sha256 логов — в JSON.

**Разбор двух падений.**
- `test-od-host`: 376 passed, 3 failed — `tests::native_register_v1_corpus::cached_*` (нет файлов `work/stage3-native-register-v1/manual/native-register-v1-{observations,cache-trace}.json` в свежем worktree). Cargo остановился на lib-цели, остальные цели пакета не запускались. Дополнительный запуск `cargo test -p od-host --locked --offline --no-fail-fast` (вне сводки, лог sha256 `60da8c54…e2e7`): все цели `passed` — lib 379 passed, `stage4_exception_delivery` 39, `stage4_memory_fault` 1, `stage3_dword_store_corpus` 5, `memory_corpus` 22 (2 ignored), `diagnostic_jit_event` 4; при этом файлы в `work/` уже были созданы первым прогоном. Вывод: тесты не гермитичны (зависят от порядка/состояния `work/`); это не регрессия этапа 4, но воспроизводимость на чистом дереве не подтверждена.
- `test-od-runner`: lib/bin 267 + 589 passed, одна цель упала — `stage4_probe_cli::uart_echo_profile_checks_rx_expectation_budgets_and_stop` (`left: Some(5)`, `right: Some(6)`, `tools/runner/tests/stage4_probe_cli.rs:303`, код выхода `pio_error`). Не дошли три цели; дополнительный запуск `--no-fail-fast --test stage4_stack_build_cli --test stage4_store_load_build_cli --test stage4_uart_irq_cli` (вне сводки, sha256 `383487fe…ec754`): 2 + 1 + 6 passed. Падение `stage4_probe_cli` не исследовано, не исправлено и остаётся открытым (владелец — следующий срез S4-44 / S4-46).

**NOT RUN (не выполнялось, не заменено локальным успехом).**
- Linux positive/negative boot — нужны vmlinuz, initramfs, ACPI manifest/blob.
- `od-runner linux-direct-gate` на настоящем ядре — те же артефакты.
- Mac mini hardware cases (blocked) — стенд и владелец; hardware gate этапа 3 остаётся в своём scope.
- Сборка guest/firmware artifacts (`tools/build/*.py`, `firmware/uefi`, `tools/build/test_*.py`) — отдельная сборка (S4-44b).
- `tests/system` smokes VMHost app — app bundle и desktop-сессия, scope этапа 3.
- Также не выполнялись из плана S4-44: runner stress mode, `--reboot-cycles`, явный лимит guest RAM, лимит executable cache (передано из S4-43).

**Ограничения набора:** нет `--no-fail-fast` (после первого падения цели пакета остаются непроверенными — исправить в S4-44b); сводка JSON отражает только один прогон скрипта, дополнительные запуски описаны выше текстом.

## S4-44b — результат

**Статус.** S4-44b выполнен: локальный набор зелёный (`overall_local` = `passed`: 8 `passed`, 1 `known_divergence` (`fmt`), 0 `failed`, 0 `timeout`). S4-44 в целом остаётся открытым: Linux boot, Mac mini, сборка guest/firmware artifacts, `tests/system` smokes не выполнялись (см. NOT RUN в разделе выше и в сводке). Ветка `s4-44b-fix` от S4-44a (`ba8bfd3`, main `47dd472`). Сводка: `docs/reports/data/stage4/s4-44b-local-set.json` (схема v2, результат по каждой цели), скрипт `tools/stage4/run_local_set.sh` (sha256 `543f9378…6d42`).

**Падение `stage4_probe_cli` (код 5 вместо 6) — вердикт: устаревший тест (не регрессия кода раннера, не окружение).** Доказательство: (1) падение воспроизведено точечно (`cargo test --test stage4_probe_cli uart_echo`): `left: Some(5)`, `right: Some(6)`, строка 303. (2) Ручной запуск `od-runner --stage4-uart-echo-probe <образ> --expect-byte 1` без `--rx-byte` даёт `retired=3`, `rip=0x400003`, `serial_bytes=[0]`, `status=completion_mismatch`, код 5: гость выполнил `in al,dx; out dx,al; hlt` и вывел 0. (3) Причина в ядре: коммит `001b79a` (S4-38L9z, 2026-11-13) осознанно изменил `uart16550.rs` — чтение RBR пустого приёмника возвращает последний принятый байт (0 после сброса), как у настоящего 16550, вместо `DeviceError::WriteOnly` (в диффе `-.ok_or(DeviceError::WriteOnly)`); тесты `od-machine` в том же коммите обновлены, тест раннера `stage4_probe_cli` (последний раз менялся в `fb49c93`, 2026-11-07) — нет. Код раннера (`stage4_probe.rs`: `PortIo` → код 6, иначе 5) корректен и не менялся; ядро не менялось. Тест теперь проверяет актуальное поведение: `--expect-byte 1` → `completion_mismatch`, код 5, `serial_bytes=[0]`; `--expect-byte 0` → `guest_completed`, код 0. Ожидание подогнано не «под результат», а под зафиксированный и задокументированный контракт UART; `git bisect` не потребовался: изменяющий коммит найден по `git log`/diff `uart16550.rs`.

**`od-host` `native_register_v1_corpus` — выбран вариант (а): фикстура воспроизводится в тесте.** Генерация корпуса (1200 входов × все backends) занимает около 1 с (`generated_corpus_runs_twice_across_all_backends`: 1.23 с). Зависимость от `work/` имели не три, а шесть тестов группы: `cached_*` (3), `retained_hit_failure_reducer_and_selfcheck`, `saved_case_round_trip_and_replay`, `reduce_saved_native_register_v1_case` (ревью нашло последние три). Теперь все они при отсутствии `native-register-v1-{observations,cache-trace,saved-case}.json` строят их детерминированно (хелперы `generate_corpus_observations`, `load_or_generate_corpus_observations`, `saved_case_path`, `build_cache_trace`) и сохраняют; существующий файл читается и валидируется, явные пути `OD_JIT_NATIVE_REGISTER_V1_*_CASE` используются как заданы. Цикл генерации корпуса вынесен в один хелпер, общий для `generated_corpus_…` и загрузчиков. Проверено на пустом `OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR`: все 9 тестов группы проходят и по отдельности, и вместе (14.9 с); без переменной — при удалённом `work/stage3-native-register-v1` два прогона подряд проходят. Вариант (б) (NOT RUN) не понадобился.

**`--no-fail-fast`.** Добавлен во все шаги `cargo test`; сводка содержит `targets[]` по каждой цели (бинарник, статус, счётчики, имена упавших тестов).

**Исправления по ревью (после первой версии отчёта).** (1) Скрытая зависимость от `work/` осталась в трёх тестах (см. выше) — исправлена, проверена на пустом каталоге. (2) Цикл генерации корпуса вынесен в общий хелпер. (3) Покрытие кода 6 (`EXIT_PIO`) на уровне CLI невозможно: загрузчик пробы принимает только фиксированный код (flat: `mov rax,[rbx]; out dx,al; hlt`, echo: `in al,dx; out dx,al; hlt`), чей COM1 с S4-38L9z не отдаёт ошибку порта; произвольный гостевой код (проверено: `mov edx,0x1234; in al,dx; hlt`, неотображённый порт) отвергается загрузчиком как `loader_error` (код 4). Ближайший сценарий — unit-тест `exit_categories_remain_distinct`: решение о коде вынесено в `exit_code()`, тест фиксирует `PortIo` → 6, `Halted`+`guest_completed` → 0, остальное → 5. (4) Абсолютных путей в сводке нет (`grep -c "/Users/"` = 0): пути в `note` и `log` относительны к корню. (5) `known_divergence` для `fmt` теперь сравнивает отпечаток диффа (файлы, число блоков, число и хеш изменённых строк) со списком `KNOWN_FMT_DIFFS`: `storage_abi_v1.rs` 1 блок / 9 строк, `tools/image-tool/tests/cli.rs` 1 блок / 5 строк; новое расхождение внутри известного файла даёт `failed` (проверено подменой хеша: `failed`). Ограничение: хеш привязан к версии rustfmt.

**Прогон.** commit `3e8467d` (дерево чистое на старте), rustc 1.99.0 (b940084d7 2026-09-28), macOS 26.7.1, arm64, 2026-11-15T02:08Z (UTC, `generated_utc` сводки); общая длительность 596 с; один полный прогон после всех правок по ревью. `git_head` в сводке — `3e8467d`, коммит самой сводки идёт следом (HEAD ≠ `git_head` по построению). Более ранние прогоны (`4e6747f`: `fmt` упал на форматировании хелперов; `f118596`: зелёный) в итог не входят.

| шаг | статус | код | длительность, с | целей (упало) | тестов passed | log sha256 |
| --- | --- | --- | --- | --- | --- | --- |
| `fmt` | known_divergence | 1 | 5 | — | — | `bf047400…` |
| `check-workspace` | passed | 0 | 1 | — | — | `9385a919…` |
| `test-od-isa` | passed | 0 | 12 | 9 (0) | 4172 | `c907607d…` |
| `test-od-machine` | passed | 0 | 75 | 31 (0) | 1438 | `6b4ba250…` |
| `test-od-jit` | passed | 0 | 1 | 2 (0) | 455 | `1e7e960c…` |
| `test-od-host` | passed | 0 | 19 | 7 (0) | 450 | `1bd6e11b…` |
| `test-od-runner` | passed | 0 | 452 | 28 (0) | 926 | `8b79f891…` |
| `test-od-bench` | passed | 0 | 6 | 5 (0) | 81 | `c44c726f…` |
| `test-rest-of-workspace` | passed | 0 | 25 | 16 (0) | 1046 | `efd7b89a…` |

`fmt` — `known_divergence`: отпечаток равен `KNOWN_FMT_DIFFS` (только `crates/od-host/src/storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs`). Короткие длительности сборочных шагов объясняются прогретым `target/`; `test-od-runner` стабильно около 450-470 с (длительность самих тестов).

**Оговорки.** (1) Независимость тестов `native_register_v1_corpus` от `work/` подтверждена отдельным точечным прогоном на пустом `OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR` (9 тестов: по отдельности и вместе), а не самим набором: в итоговом прогоне `work/stage3-native-register-v1/manual/` в worktree уже существовал. (2) Зелёный локальный набор не закрывает S4-44: NOT RUN-пункты не изменились (Linux positive/negative boot, `linux-direct-gate` на настоящем ядре, Mac mini — blocked, сборка guest/firmware artifacts, VMHost smokes; runner stress mode, `--reboot-cycles`, лимит guest RAM, лимит executable cache). (3) Сводка `s4-44a-local-set.json` сохранена как исторический результат S4-44a. (4) Коммиты `6265daa` и `4e6747f` по отдельности не проходят `cargo fmt --check` (форматирование исправлено позже, `f118596`); дерево на `3e8467d` и позже проходит.

## S4-45 — независимое итоговое ревью (выполнено Opus, не Sol/Astra)

**Статус.** Ревью проведено моделью Opus с уровнем усилия high, режим read-only,
по всему этапу 4 (код, контракты, отчёты, данные, статусы документов). Внешние
модели `gpt-6.1-sol` и `gpt-6-astra`, названные в плане S4-45, недоступны в
среде; ревью ими **не проводилось**. Пункт S4-45 в плане не отмечен как
«принято; все обязательные замечания закрыты»: остаточные ограничения не
закрыты, а переданы в реестр.

**Что это значит для более ранних записей.** Заявления «независимое ревью
Sol/Astra» в более ранних срезах (например, S4-20e и другие разделы
[плана](../plans/STAGE_4.md) и [CPU_CONTRACT](../specs/CPU_CONTRACT.md))
не относятся к ревью S4-45, а их происхождение этим ревью не проверялось. Они
сохранены без изменений как записи тех срезов; ссылаться на них как на
независимое итоговое ревью нельзя.

**Что учтено в документах по результатам ревью (S4-46).**
- Текущий статус переписан: доказано / не доказано (блок «Текущий статус»).
- Добавлены разделы Linux direct boot, S4-40, S4-41, S4-45, S4-46.
- Отклонения от плана S4-39/S4-40/S4-41/S4-43 помечены в плане, статусы
  принятых срезов не менялись.
- В [KNOWN_ISSUES](../KNOWN_ISSUES.md) заведены записи с владельцами и
  следующим шагом (ISSUE-090…ISSUE-099), ISSUE-088 получила актуальный статус.
- NOT RUN-пункты S4-44 дополнены (runner stress mode, `--reboot-cycles`,
  явный лимит guest RAM, лимит executable cache); Mac mini разделён на KVM-прогон
  (S4-34a/b/cde выполнен) и bare-metal (blocked).
- Добавлен раздел воспроизведения Linux direct boot.

Полный текст ревью в репозитории не хранится; выше перечислено только то, что
отражено в документах. Новых прогонов, кода и тестов в рамках S4-46 не
запускалось.

## S4-46 — итог этапа 4 (частичный)

**Вердикт.** Этап 4 закрыт **частично**, не завершён. Linux userspace локально
подтверждён на пути интерпретатора под CPUID-профилем stage4a для узкого
сценария `/init`; S4-41, S4-42, S4-43 приняты с ограничениями (родитель S4-42 по
плану остаётся открытым); S4-44 открыт (часть NOT RUN); S4-45 выполнен как
независимое ревью Opus (внешние модели gpt-6.1-sol / gpt-6-astra недоступны в
среде); S4-46 — частичный итог. Это не заявление о стабильности Linux и не
основание считать этап 4 готовым для этапа 5: чекбокс S4-46 в плане не
отмечен, передача зависимостей этапам 5 и 6 не оформлена.

**Что доказано (локально, интерпретатор, stage4a).**
- S4-39j: два холодных зачётных прогона `PASS`, run ID
  `9d34785cdd85762428ba69873dd543700d87a50af5391e49d1728e79709b92c2`, отчёты
  побайтно равны, `counted: true` (раздел «Linux direct boot (S4-38/S4-39)»).
- Гейт не даёт ложного `PASS` на негативах S4-40 (с пробелами).
- Lifecycle `od-runner linux-direct` и подпроцесс в VMHost (S4-41a/b, с
  ограничением).
- Микробенчмарк трансляций (S4-42a/b/c) и ограниченный churn/reset
  трансляций, кэша и очередей (S4-43a/b/c).
- Локальный воспроизводимый набор зелёный (S4-44a/b): 98 целей, 8568 тестов,
  `fmt` — `known_divergence`.

**Что не доказано и не закрыто.**
- Live IRQ/timer в CPL3 на физическом Linux-госте (S4-39h прошёл только
  synthetic board integration test);
  stage4a без APIC и SSE/SSE2; nonce фиксированный `s4-38l4a`; JIT на Linux 0 %;
  pause/resume для Linux нет; control v2 не несёт путь к ядру; живой serial и
  доставка `od-runner` в бандле; гейт не проверяет `exit_kind`,
  `cpuid_profile`, timer IRQ; `counted` для stage4a — решение пользователя без
  записанного ревью; ISSUE-088 не проверена при больших квантах.
- S4-44 NOT RUN: Linux positive/negative boot и `linux-direct-gate` на настоящем
  ядре (артефакты не в репозитории), сборка guest/firmware artifacts, VMHost
  smokes, runner stress mode, `--reboot-cycles`, явный лимит guest RAM, лимит
  executable cache; Mac mini bare-metal — blocked (KVM-прогон S4-34a/b/cde
  выполнен и не является bare-metal).
- Не выполнено по самому S4-46: ссылки из `ROADMAP.md`, `PROJECT_LAYOUT.md`,
  `ARCHITECTURE.md` и support matrix не обновлялись; передача зависимостей
  этапам 5 (storage/network, устойчивые workloads) и 6 (SMP/ordering) не
  оформлена. Это тоже отклонение от плана S4-46.
- Реестр: [KNOWN_ISSUES](../KNOWN_ISSUES.md), ISSUE-088 (статус обновлён) и
  ISSUE-090…ISSUE-099 (владельцы и следующие шаги указаны в записях).
- Воспроизведение: `docs/testing/MACHINE.md`, раздел «Воспроизведение Linux
  direct boot (S4-39j)»; без vmlinuz и initramfs воспроизводится только гейт на
  сохранённых отчётах.

**Решение, которое требуется от владельца.** Принять ли частичное закрытие
этапа 4 с передачей остатков (записи ISSUE-088, ISSUE-090…ISSUE-099, NOT RUN
S4-44) этапу 5 и владельцам срезов, либо продолжить этап 4: G3-гейты
(недостающие негативы и проверки гейта: `exit_kind`, `cpuid_profile`, timer
IRQ, новый nonce на прогон), IRQ из CPL3 (S4-39h) и stage4a с SSE/SSE2 и APIC.
До этого решения этап 4 в документах не называется завершённым.

**Ветки и worktree для решения о слиянии/удалении (только список, ничего не
удалялось и не сливалось).**
- `feat/s4-06c-paged-mov-mmio` — устарела, содержимое в main; кандидат на
  удаление.
- `feat/s4-14b-invlpg` — устарела, содержимое в main; кандидат на удаление.
- `codex` — 21 коммит, не слитые в main; ветка не принадлежит этому потоку,
  не трогать.
- worktree `agent-aea7419902481de3a` — грязный дубль S4-39f4; кандидат на
  удаление после проверки, что в нём нет уникальной работы.

**Ссылки.** [План этапа 4](../plans/STAGE_4.md) (пометки S4-46),
[CPU_CONTRACT](../specs/CPU_CONTRACT.md) (S4-39j, S4-45),
[MACHINE](../testing/MACHINE.md), данные `docs/reports/data/stage4/s4-39j-gate-*.json`,
`docs/reports/data/stage4/s4-44b-local-set.json`.

## S4-23f — арбитр простоя для N vCPU (не подключён; S4-23 открыт)

- **Файлы:** новый `crates/od-machine/src/idle_arbiter.rs` и строка `mod idle_arbiter;` в `crates/od-machine/src/lib.rs`. Код `od-isa`, плата, pump и `virtual_clock.rs` не менялись.
- **Что сделано:** чистая функция `decide(now, stop, views, board_deadlines)` возвращает `Stopped | Runnable(idx) | WakeNow(idx) | AdvanceTo(ns) | AllIdle | NoProgress`; `apply(now, to_ns, advance, views)` продвигает часы ровно до срока одним вызовом `advance(to_ns - now)` и затем один раз вызывает `dispatch_timer` у каждого vCPU по индексу. Порядок: STOP, Running, eligible (Halted, IF=1, вектор принимается), минимум полезных сроков. Правило полезного срока повторяет `useful_timer_deadline` pump (ISSUE-070); сроки платы ждутся независимо от IF; ничьи разрешаются по индексу; нет HashMap-итерации и host-времени.
- **Тесты:** 15 unit-тестов `idle_arbiter` прошли (срок таймера vCPU1 будит только его; равные сроки: один dispatch на vCPU по индексу; IF=0 игнорирует таймер; заблокированный TPR/ISR вектор не ждётся; WaitForSipi и Shutdown не выбираются; нет сроков → AllIdle; срок ≤ now → NoProgress; STOP не меняет время; переполнение часов (TSC) не меняет часы и очереди; отказ dispatch у vCPU1: часы продвинуты, vCPU0 обработан, состояние vCPU1 не изменилось, vCPU2 не обработан (отказ получен прямой правкой `global_enabled`, т.к. публичное `set_global_enabled(false)` сбрасывает таймер и отказа не даёт); повтор прогона даёт тот же результат; сроки платы независимы от таймеров; Running раньше всего, кроме STOP). На одном vCPU четыре сценария (таймер, таймер при IF=0, без таймера, вектор заблокирован TPR) сверены с реальным `InterruptPump::idle_advance` через тестовый backend: совпали исход (Budget/IdleHalt), время часов, очередь таймера и IRR/ISR.
- **Ограничения:** арбитр не подключён ни к одному рабочему пути (код помечен `allow(dead_code)`); решение D7 здесь не принималось; `od_isa::CpuActivity` не содержит WaitForSipi, поэтому в отображении такого состояния нет, ветка арбитра его никогда не выбирает; NMI, INIT и SIPI как выход из сна, host wait, темп времени при Running, захват `VirtualClockAdvancer` платой и планировщик не реализованы. Сроки очереди IRQ (route) в views не входят. Контракт `board_deadlines`: арбитр ждёт каждое значение независимо от IF и IRR (как срок хоста в pump), а срок очереди в pump маскируемый (только при IF=1 и если вектор не в IRR); поэтому вызывающий передаёт только сроки, чей целевой vCPU в Halted с IF=1 и чей вектор не в IRR этого LAPIC, либо сроки немаскируемых источников (работа хоста/владельца). Арбитр это не проверяет; очередь не входит в маскируемую половину `idle_advance`, с которой сверено только правило таймера. Срок не позже `now` даёт `NoProgress`, потому что вызывающий обязан сначала выполнить dispatch просроченных таймеров, как делает pump. При отсутствии halted vCPU арбитр возвращает `AllIdle`, не продвигая время ради сроков платы. Ошибка dispatch у vCPU N после продвижения часов возвращается как `ApplyError::Dispatch`, часы остаются продвинутыми.

## S4-23g — единый владелец часов и пробуждение из HLT по сроку (не подключён; S4-22 и S4-23 открыты)

- **Файлы:** новый `crates/od-machine/src/clock_owner.rs` и строка `mod clock_owner;` в `crates/od-machine/src/lib.rs`. `idle_arbiter.rs`, `virtual_clock.rs`, `local_apic.rs`, `firmware_board.rs`, `interrupt_pump.rs` и `od-isa` не менялись.
- **Что сделано:** `ClockOwner::new(&VirtualClockHandle, Vec<LocalApicHandle>)` один раз берёт `claim_advancer` (владение постоянное; повторный claim, `ClockOwner::new` и `advance_standalone` дают `ClockAlreadyOwned`). `account_retired(retired)` двигает часы на `retired * NS_PER_RETIRED` (константа 1 нс, решение D7: гость видит только виртуальное время) с проверкой переполнения; при ошибке часы и очереди не меняются; затем `dispatch_timer` каждого LAPIC с таймером по порядку индексов. `idle_step(stop, views, board_deadlines)` вызывает `idle_arbiter::decide`; при `AdvanceTo` вызывает `idle_arbiter::apply` с `|d| advancer.advance_time_ns(d)` и повторно `decide`. Результат `IdleStep { outcome, idle_ns, fired }`, где `outcome` равен `Woken(idx) | AllIdle | Stopped | NoProgress | Runnable(idx) | Advanced(next)`. Внутри нет цикла и ожидания: один вызов делает не больше одного прыжка часов. `Woken(idx)` означает только «можно перевести в Running» (вектор в IRR и eligible), доставки через IDT нет. Активность vCPU ведёт вызывающий через `SchedActivity`; `state.activity` не используется.
- **Отличия от задания:** к списку исходов добавлены `Runnable(idx)` (решение арбитра, когда какой-то vCPU работает) и `Advanced(next)` (часы прыгнули, но никого ещё нельзя будить, например вектор заблокирован TPR, а таймер другого vCPU позже); без них повторное решение пришлось бы терять или сводить к неверному исходу. После прыжка к сроку платы повторный `decide` с тем же списком даёт `NoProgress`, пока вызывающий не уберёт потреблённый срок (описано в доке и закреплено тестом).
- **Тесты (8, все прошли):** `two_vcpus_one_halted_timer_clock_jumps_exactly_to_deadline_and_wakes_only_it` (now равен retired_total и TSC, затем ровно T; fired `[true,false]`; `Woken(0)`; vCPU1 в Halted с пустым IRR; побайтные записи в RAM; повторный вызов не двигает время); `deadline_crossed_while_the_other_vcpu_runs_wakes_at_the_boundary_without_idle_skip` (IRR выставлен на первой границе после срока, `idle_ns == 0`, now равен числу retired); `same_inputs_give_byte_identical_trace` (трасса now, решений, retired/RIP, RFLAGS, IRR и RAM совпадает; срок T+1 и другая длина программы дают другую трассу); `the_owner_is_the_only_advancer`; `a_refused_jump_in_idle_step_changes_neither_clock_nor_queues` (TscOverflow при прыжке, часы и очередь не изменены); `no_deadline_or_stop_ends_in_one_call` (AllIdle и Stopped, часы стоят, таймер остаётся взведён); `a_board_deadline_jump_reports_no_progress_until_the_caller_consumes_it`; `a_later_timer_of_another_vcpu_needs_a_second_call_not_a_loop`. Тесты исполняют настоящий `od_isa::execute` на identity RAM и `LocalApic::with_shared_timers(clock, HZ, 2)`; таймер взводит harness. Мутация `NS_PER_RETIRED = 2` ломает 5 из 8 тестов.
- **Особенность исполнения:** обычный `od_isa::execute::run` не исполняет HLT, а профиль `run_with_halt_profile` допускает только `hlt` (и `sti`/`cli`/`hlt`), поэтому harness исполняет заполнители `mov [rbx], rax` через `run` до адреса `hlt`, а сам `hlt` через `run_with_halt_profile`. `od-isa` не менялся.
- **Ограничения:** владелец не подключён к плате, pump, планировщику и `od-isa` (код помечен `allow(dead_code)`); нет квантов, seed и расписания (S6-15); нет доставки вектора через IDT, NMI/INIT/SIPI, host wait и тиков PM/PIT/RDTSC; часы платы владелец не берёт, в тестах он живёт до конца сценария; `dispatch_timer` поднимает не больше одного запроса за вызов: периодический таймер склеивает пропущенные периоды в одно прерывание с сохранением фазы (как S4-28b), one-shot срабатывает на границе шага.
- **Условие для среза подключения (обязательное):** вызывающий ограничивает бюджет шага vCPU ближайшим сроком, то есть `(next_deadline - now) / NS_PER_RETIRED`; иначе число тиков периодического таймера и момент прерывания зависят от размера кванта. S4-23 нельзя закрывать без этого условия.
- **Контракт `idle_step`:** `views` содержит те же LAPIC в том же порядке, что список владельца (`debug_assert_eq!` на длину): `account_retired` диспатчит по списку владельца, прыжок `idle_step` по `views[i].apic`. `account_retired` и `idle_step` берут `&mut self`, чтобы чтение `now` и продвижение часов шли под одним исключительным заимствованием. `NoProgress` различается по `idle_ns`: 0 означает, что часы не двигались (устаревший срок), больше 0 означает прыжок к сроку платы, который вызывающий должен убрать из списка.

## S4-23h — ограничение бюджета шага vCPU ближайшим сроком (не подключено; S4-22 и S4-23 открыты)

- **Файлы:** только `crates/od-machine/src/clock_owner.rs`. `idle_arbiter.rs`, `local_apic.rs`, `virtual_clock.rs` и `od-isa` не менялись.
- **Что сделано:** чистая функция `capped_budget(quantum, now, deadline)` (через `budget_with_rate`, где `rate` равен `NS_PER_RETIRED`) и методы `ClockOwner::nearest_deadline(board_deadline)` (минимум `next_timer_deadline` всех LAPIC владельца и одного срока платы) и `ClockOwner::step_budget(quantum, board_deadline)`. Правило: без срока бюджет равен кванту; иначе `min(quantum, (deadline - now) / NS_PER_RETIRED)`; результат никогда не 0. Срок не позже `now` (устаревший или уже наступивший) и нулевой квант дают 1: шаг не пустой, цикл без прогресса невозможен, перерасход ограничен одной инструкцией. Явной ошибки нет (выбор записан здесь). Разрыв меньше цены одной инструкции округляется вниз до 0 и поднимается до 1; при D7 (1 нс) такого нет, это лишь запас для другой ставки.
- **Тесты (4 новых, 12 из 12 `clock_owner` прошли):** `tick_count_does_not_depend_on_quantum` (два vCPU на `od_isa::execute::run` с циклом `jmp -2`, периодические таймеры 100 и 70 нс, горизонт 1050 нс задан как срок платы; кванты 1, 7 и 1000 дают одну и ту же трассу `(vCPU, now)` из 25 срабатываний, ровно на кратных 100 и 70; без правила те же программы дают другую трассу, и кванты 7 и 1000 различаются между собой); `budget_never_crosses_a_deadline` (кванты 1, 7, 64, 1000: ни один шаг не уходит дальше ближайшего срока, при квантах больше 1 правило действительно обрезало шаги); `no_deadline_budget_is_quantum` (без таймеров, при замаскированном таймере и после ретайрмента бюджет равен кванту, включая `u64::MAX`; затем срок таймера и срок платы ограничивают бюджет, берётся ближайший); `budget_is_never_zero` (срок равен, меньше или на 1 больше `now`, нулевой квант, разрыв меньше ставки; на настоящих часах владельца после 10 retired).
- **Мутации:** замена `quantum.min(deadline.saturating_sub(now) / rate)` на `quantum` ломает 4 теста (`tick_count_does_not_depend_on_quantum`, `budget_never_crosses_a_deadline`, `no_deadline_budget_is_quantum`, `budget_is_never_zero`); снятие `.max(1)` ломает `budget_is_never_zero`. Обе мутации откатаны.
- **Закрыто:** обязательное условие S4-23g («вызывающий ограничивает бюджет шага ближайшим сроком») закрыто для тестового harness: правило реализовано, зафиксировано в коде владельца и доказано тестами на настоящем исполнении `od-isa`.
- **Не закрыто:** подключение к плате и pump (`step_budget` не вызывается ни одним рабочим путём, модуль по-прежнему под `allow(dead_code)`), планировщик, кванты и seed (S6-15), доставка вектора через IDT, host wait. Подключение и pump остаются следующим срезом вместе с S6-15; S4-22 и S4-23 нельзя закрывать до него.


**Условия среза подключения (из ревью S4-23h, обязательны до закрытия S4-22/S4-23):**
- Наступивший срок платы вызывающий обязан обработать и убрать до следующего `step_budget`. Иначе бюджет всегда равен 1 нс и шаги идут по одной инструкции (деградация производительности, не корректности).
- Квант 0 недопустим: при `quantum == 0` и отсутствии срока `step_budget` возвращает 1, а не квант. Вызывающий (планировщик S6-15) не должен обозначать нулевым квантом «не запускать».
- Гостевая запись в LAPIC (initial count, LVT) внутри шага создаёт срок, который бюджет, посчитанный до шага, не учитывает. Нужно либо завершать шаг на MMIO-доступе к LAPIC, либо пересчитывать бюджет. Наследуется от S4-23g; в harness гость к LAPIC не обращается.
- Горизонт `HORIZON = 1050` в тесте `tick_count_does_not_depend_on_quantum` совпадает со сроком таймера vCPU1, поэтому срок платы там не обрезает шаг сам по себе. Для проверки срока платы нужен горизонт, не совпадающий с таймерами (например, 1060).

## S4-23i — сквозной тестовый цикл двух vCPU: простой, часы, бюджет шага (не подключено; S4-22 и S4-23 открыты)

- **Файлы:** новый `crates/od-machine/src/interleaved_harness.rs` (модуль `#[cfg(test)]`, в сборку не входит) и строка `#[cfg(test)] mod interleaved_harness;` в `crates/od-machine/src/lib.rs`. `clock_owner.rs`, `idle_arbiter.rs`, `firmware_board.rs`, `interrupt_pump.rs`, планировщик S6-15 и `od-isa` не менялись; существующие тесты не менялись.
- **Что сделано:** `Interleaved::run_until(quantum)` крутит N vCPU по кругу на одних виртуальных часах (D7: 1 retired = 1 нс, время только виртуальное). Итерация: выход на горизонте; снятие наступивших сроков платы; пробуждение halted vCPU с IF=1 и eligible вектором (вектор берётся и завершается сразу, IDT не моделируется); шаг следующего `Running` vCPU по кругу с бюджетом `ClockOwner::step_budget(quantum, min(срок платы, горизонт))` на настоящем `od_isa::execute::run` и `account_retired`; если никто не работает, один вызов `ClockOwner::idle_step` (один прыжок часов, без ожидания). Цикл ограничен горизонтом и счётчиком итераций (assert при зацикливании). Пробуждение при работающем другом vCPU делает сам цикл (работа планировщика, арбитр в этом случае отвечает `Runnable`).
- **Условия S4-23h:** (a) срок платы, который наступил, цикл убирает из списка до следующего `step_budget` (в тесте это видно по списку и журналу `consumed`); (b) `debug_assert!(quantum > 0)` в `run_until`, `step_budget` по-прежнему возвращает 1 при нуле (выбор: паника в debug, а не возврат ошибки, нулевой квант считается ошибкой вызывающего); (c) гость к LAPIC в harness не обращается (таймеры взводит harness между шагами), ограничение записано комментарием в модуле, теста нет; (d) горизонты тестов (1060, 700060) не совпадают ни с таймерами (700, 700000), ни со сроком платы (850).
- **Тесты (4 новых, 4 из 4 прошли):** `hlt_wake_by_timer_without_polling` (vCPU0 в HLT с таймером на T, vCPU1 исполняет 20 заполнителей и HLT; в момент первого простоя часы равны retired_total = 24, затем ровно T; просыпается только vCPU0, vCPU1 остаётся Halted с пустым IRR; vCPU0 исполняет второй сегмент и снова засыпает; срок платы 850 убран ровно на 850; прогоны с T=700 и T=700000 (простой отличается в 1000 раз) дают одно и то же число вызовов `idle_step` (3) и итераций цикла, то есть polling нет); `board_deadline_is_consumed_and_does_not_shrink_steps_forever` (vCPU1 крутится, таймер vCPU0 наступает во время его шага: пробуждение ровно на 700, шаг заканчивается ровно на 850, срок убран, после этого каждый бюджет равен `min(quantum, horizon - now)`, шагов после 850 меньше 30; ни один шаг не пересекает 700 и 850; отрицательный контроль с `consume_due = false` даёт 210 шагов по одной инструкции); `trace_is_byte_identical_for_same_inputs` (трасса now, решений арбитра, бюджетов, retired и RIP по vCPU, пробуждений и RAM, 16 КиБ, совпадает в двух прогонах; сдвиг таймера или срока платы, а также их отсутствие дают другую трассу, в том числе в сценарии со spin); `quantum_zero_is_rejected_in_debug` (`#[should_panic]` под `cfg(debug_assertions)`).
- **Мутации:** замена `quantum.min(deadline.saturating_sub(now) / rate)` на `quantum` в `capped_budget` ломает 2 из 4 (`board_deadline_is_consumed_and_does_not_shrink_steps_forever` и `trace_is_byte_identical_for_same_inputs`); `hlt_wake_by_timer_without_polling` эту мутацию не ловит (там все vCPU спят до срока, прыжок делает арбитр), её роль: часы равны retired_total до прыжка, ровно T после, нет polling. Мутация откатана (файл восстановлен, git diff по clock_owner.rs пуст). Отключение снятия срока платы показывает отрицательный контроль (`consume_due = false`).
- **Не закрыто:** подключение к плате, pump, `firmware_board.rs` и планировщику S6-15 (квант, seed, расписание), доставка вектора через IDT, гостевая запись в LAPIC внутри шага (нужен конец шага на MMIO LAPIC или пересчёт бюджета), NMI/INIT/SIPI, host wait. S4-22 и S4-23 остаются открытыми до подключения к рабочему циклу.

## S4-23j — подключение владельца часов и арбитра к рабочему циклу платы (opt-in; S4-22 и S4-23 открыты)

- **Файлы:** `crates/od-machine/src/firmware_board.rs` (поле `clock_driver: BoardClock { Standalone, Owned(ClockOwner) }`, `enable_clock_owner`, `charge_ns`/`charge_retired`/`idle_to`, правки цикла `run_linux_boot_cpl0_set`), `crates/od-machine/src/clock_owner.rs` (новый `account_device_ns`, общий `dispatch_all`, документация D7), `crates/od-machine/src/linux_direct.rs` (член `LinuxCpl0SetMembers::clock_owner`). `idle_arbiter.rs`, `interrupt_pump.rs`, `od-isa` и runner не менялись.
- **Что сделано:** по умолчанию плата `Standalone` и цикл идёт прежним кодом (часы двигает сама плата, `advance_standalone`; владелец не создаётся, часы не занимаются). `enable_clock_owner` (идемпотентный) занимает продвигатель часов и берёт LAPIC всех vCPU по порядку; отказ без изменений, если PIT или таймер LAPIC собраны на чужих часах (`foreign_timer_clock`, тестовая инъекция) или часы уже заняты. При `Owned` через владельца идут: retired (`account_retired`), тики PM, PIT, RDTSC в прежнем порядке (`account_device_ns`: сдвиг часов на готовые наносекунды без ставки и диспетчеризация таймеров LAPIC; при переполнении ничего не меняется), прыжок простоя HLT (`idle_step` с BSP `Halted`, остальные `WaitForSipi`, срок платы `wake_ns`; ожидается `NoProgress` с `idle_ns > 0`; срок `<= now` даёт прыжок 0) и бюджет шага (`step_budget(remaining, next_deadline_ns())`, `cut_by_deadline = бюджет < remaining`, счётчик `quantum_cut_by_deadline` растёт по тому же условию). `next_deadline_ns` и `wake_ns` вызываются ровно столько раз и в тех же точках, что и раньше (в том числе `next_deadline_ns` при `remaining == 0`, но `step_budget` туда не передаётся, плюс `debug_assert`). `Woken` по вектору LAPIC при источнике PIC при простое даёт типизированный отказ (`FirmwareBoardError::Config`, прогон `run_failed`), иначе гость прошёл бы HLT без прерывания. Включение: член `clock_owner` (`false` в `minimal()` и `linux_default()`, имя в `set_members` только при `true`), поэтому манифесты и записи по умолчанию не меняются. После включения публичный `advance_time_ns` даёт `ClockAlreadyOwned`.
- **Условия S4-23h:** (a) срок платы потребляется в той же итерации (после прыжка он снимается, цикл не передаёт его повторно); (b) `remaining == 0` в `step_budget` не передаётся, `debug_assert!(1 <= бюджет <= remaining)`; (c) LAPIC MMIO в set-loop нет, сторожевой тест `the_set_loop_has_no_local_apic_mmio` (чтение и запись регистров LAPIC через диспетчер set-loop отвергаются); (d) горизонты тестов (8381 PIT, 9000 LAPIC) не совпадают со сроками.
- **Побайтное равенство умолчания:** существующие тесты S4-39g/i и golden `board_refactor_golden_handoff_manifest_hash_tables_and_entry_state` проходят БЕЗ изменения ожиданий (полный прогон `od-machine`: 1240 passed, 6 ignored, 0 failed; `cargo test -p od-runner --offline`: все целевые тесты прошли, в том числе 267 unit-тестов бинарника и 589 в интеграционном наборе, 0 failed). Ветка `Standalone` в цикле сохранена как была.
- **Тесты (все прошли):** `opt_in_matches_default_for_sti_hlt_woken_by_ir0` (два борта на одном образе: равны `LinuxSetRunReport`, state, COM1, часы, PIC; варианты: пробуждение IR0, 20 000 инструкций с периодическим IR0, второй HLT при IR0 в обслуживании, IF=0, IR0 замаскирован), `opt_in_matches_default_for_the_uart_ir4_run` (IR4 UART через фасад user-mode, записи равны с точностью до имени члена), `default_never_claims_the_clock`, `opt_in_quantum_does_not_change_wake_times` (CHUNK 311 и 29 против целого прогона, с владельцем и без: одна трасса часы/пробуждения/доставки), `opt_in_host_armed_lapic_timer_cuts_quantum_and_clock_stays_single` (срок LAPIC 9000, retired и часы те же, владелец режет больше квантов, вектор LAPIC в IRR, при владельце `advance_time_ns` даёт `ClockAlreadyOwned`), `opt_in_refuses_a_lapic_vector_that_would_wake_the_cpu`, `opt_in_zero_budget_executes_nothing`, `the_set_loop_has_no_local_apic_mmio`, `the_user_mode_facade_names_the_member_only_when_it_is_on`, `opt_in_refuses_foreign_timer_clock`, `opt_in_takes_the_clock_once_and_refuses_when_somebody_owns_it`, `device_ns_move_the_clock_without_a_rate_and_dispatch_the_timers`.
- **Мутация:** в `ClockOwner::step_budget` убран срок (`self.nearest_deadline(board_deadline)` заменён на `None`): падают 4 теста (`opt_in_matches_default_for_sti_hlt_woken_by_ir0`, `opt_in_quantum_does_not_change_wake_times`, `opt_in_host_armed_lapic_timer_cuts_quantum_and_clock_stays_single`, `the_user_mode_facade_names_the_member_only_when_it_is_on`). Мутация откатана копированием из сохранённой копии, `cmp` совпал.
- **Ограничения среза (по ревью Opus):** отказы выражены `FirmwareBoardError::Config(&'static str)`, отдельный типизированный вариант отложен до S4-38; при `wake <= now` прыжок равен 0 и арбитр не вызывается, поэтому проверка `Woken` (вектор LAPIC при простое) пропускается, пробуждение идёт по источнику платы как в `Standalone` (мотивировка в doc `enable_clock_owner`); `quantum_cut_by_deadline` при `Owned` растёт и по сроку таймера LAPIC, а не только по сроку источника платы; `#![cfg_attr(not(test), allow(dead_code))]` в `clock_owner.rs` стоит на уровне модуля, сузить позже; после включения владельца `run_boot`, `run_boot_with` и `run` падают на `advance_time_ns` (`ClockAlreadyOwned`), Linux-пути их не вызывают.
- **Не закрыто:** Linux-видимая частота и тики PM/PIT/RDTSC (S4-38); таймер LAPIC как источник пробуждения (S4-38: сейчас вектор LAPIC при простое отвергается); N vCPU и квант (S6-15); включение флага в раннере и в зачётных прогонах не сделано (флаг только в API). Чекбоксы S4-22 и S4-23 не отмечены; независимое ревью среза не проведено.

## S4-14h — опциональная политика инвалидации кэша выборки, управляемая гостем (не подключено; S4-14 открыт)

- **Файлы:** `crates/od-machine/src/fetch_translation_cache.rs` (`FetchCacheInvalidation { WriteWatch, GuestOnly }`, `with_invalidation`, счётчики `invlpg_flushes`, `control_flushes`, `ignored_table_writes`, методы `flush_after_invlpg` и `flush_after_control_change`), `crates/od-machine/src/paged_memory_adapter.rs` (`run_cpl0_set_paged_2m_ram_with_policy`, подглядывание `invlpg_at_rip`; `run_cpl0_set_paged_2m_ram_with_cpuid_boundary` осталась обёрткой над `WriteWatch`), тесты в `crates/od-machine/src/linux_direct.rs` рядом с тестами S4-13, `docs/specs/CPU_CONTRACT.md` (раздел S4-13: исправлен пункт (2), добавлен абзац S4-14h), строка статуса в `docs/plans/STAGE_4.md`. `crates/od-isa/src/`, `paged_pump_backend`, тестовая сессия S4-14a-g (`translation_probe_cache.rs`) и места вызова старой функции не менялись.
- **Что сделано:** в `GuestOnly` запись в кадр таблицы ничего не сбрасывает, а только считается. Гостевые сбросы всего кэша: после успешно выполненного `INVLPG m` (подглядывание перед шагом по образцу `iretq_at_rip`, только чтение: префиксы `64`/`65` и REX без W, затем `0F 01`, ModRM reg=7, mod≠3; при нулевом retire кэш сохраняется), после `MOV CR3` (как раньше, в обеих политиках; тем же значением тоже), при любом изменении `state.cr0`/`state.cr4` за шаг (CR4.PGE) и при каждом `#PF`, доставленном гостю через IDT (правка по ревью: архитектура требует инвалидировать только трансляцию адреса ошибки, SDM Vol. 3A §4.10.4.1, ссылка по памяти, с первоисточником не сверена; полный сброс — намеренное надмножество, допустимое, и тест `guest_only_a_delivered_page_fault_drops_the_cached_translation` закрепляет именно эту более строгую реализацию, а не архитектурное требование; счётчик `fault_flushes`). Кэш сохраняется во всех путях без сбрасывающего эффекта: отклонённая инструкция, ошибка без доставки исключения, доставка не-`#PF` исключения (`#GP(0)`), доставка прерывания и исполненная инструкция без retire-эффекта сброса. Подглядывание и сравнение CR выполняются только в `GuestOnly`: `WriteWatch` не менялся ни по поведению, ни по счётчикам. Кэш по-прежнему создаётся заново в каждом вызове.
- **Исправление контракта:** прежний текст CPU_CONTRACT утверждал, что `INVLPG` и CR0/CR4/EFER сбрасывают кэш S4-13. Это не так: `INVLPG` в наборе CPL0 — пустая операция (S4-38L7ab), `MOV CR0/CR4` не дают `TranslationContextChange`. Текст исправлен; сброс по ним есть только в `GuestOnly`.
- **Тесты (новые):** `guest_only_invlpg_after_a_pte_store_runs_the_new_code`; `guest_only_without_invlpg_keeps_the_stale_translation_and_write_watch_does_not` (оба исхода: `GuestOnly` видит старый код A, `WriteWatch` новый B); `write_watch_ignores_the_invlpg_of_the_guest`; `guest_only_reloading_cr3_with_the_same_value_runs_the_new_code`; `guest_only_toggling_cr4_pge_runs_the_new_code` (PGE 1, 0, 1); `guest_only_rewriting_cr0_runs_the_new_code`; `guest_only_a_cr4_write_of_the_same_value_flushes_nothing`; `guest_only_two_mib_leaf_made_nx_faults_exactly_after_invlpg` (PFEC 0x11) и `guest_only_two_mib_leaf_made_not_present_faults_exactly_after_invlpg` (PFEC 0x10) — без `INVLPG` `GuestOnly` допускает продолжение по кэшу, `WriteWatch` даёт fault сразу после записи PDE; `the_bool_entry_point_is_the_write_watch_cache`; юнит-тесты `fetch_translation_cache::tests::guest_only_counts_a_table_write_and_keeps_every_entry_until_a_guest_flush` и `write_watch_is_the_default_policy_and_ignores_nothing`. После ревью добавлены: `guest_only_a_delivered_page_fault_drops_the_cached_translation` (код на X закеширован, `store` в неотображённую страницу даёт `#PF`, обработчик через IDT меняет PTE страницы X без `INVLPG` и возвращается; после рестарта исполняется код F2, а не F1; для `WriteWatch` результат тот же, `fault_flushes` = 0); `guest_only_prefixed_invlpg_forms_run_the_new_code` (`65 0F 01 38`, `41 0F 01 3F`, `64 41 0F 01 3F` сбрасывают кэш); `the_invlpg_peek_recognizes_exactly_the_decoded_shapes` (таблица из 26 форм: `SWAPGS` `0F 01 F8`, `RDTSCP` `0F 01 F9`, `0F 01 C1`, `0F 01 FB` и другие `mod = 3`, `LGDT`/`LIDT`/`SGDT`/`SMSW`, REX.W, `66`/`F0`/`26`, REX не перед опкодом — не `INVLPG`).
- **Мутации (каждая откатана, файлы совпадают с копиями до мутации):** отключение сброса по `INVLPG` ломает 3 теста (remap и два теста 2 MiB-листа); отключение сброса по `MOV CR3` ломает `guest_only_reloading_cr3_with_the_same_value_runs_the_new_code` и `cache_mov_cr3_drops_the_translations_of_the_old_tree`; сравнение только CR0 ломает тест PGE; сравнение только CR4 ломает тест CR0; сброс по записи PTE в `GuestOnly` ломает 5 тестов, в том числе юнит-тест и тест без `INVLPG`. После ревью: отключение сброса по доставленному `#PF` ломает `guest_only_a_delivered_page_fault_drops_the_cached_translation`; удаление пропуска префикса `64`/`65` или REX ломает `guest_only_prefixed_invlpg_forms_run_the_new_code` и таблицу форм; удаление проверки `mod != 3` ломает таблицу форм. Все мутации откатаны, файлы сверены с копиями побайтно.
- **Не закрыто:** кэш не постоянный между вызовами (S4-14i); нет кэша данных, постраничного `INVLPG`, PCID, SMP; `GuestOnly` ни в раннер, ни в Linux-плату не подключён. Независимое ревью (Claude Opus, high, коммит `8756a00`, 2026-11-15): принято с ограничениями, P0–P2 нет; P3 закрыты правкой формулировок (комментарий теста, CPU_CONTRACT, этот отчёт). Открытые P3-пробелы тестов: (а) нет теста на INVLPG без retire (например, на CPL3 с `#GP`) и на сохранение кэша при `#GP(0)`, поэтому условие `instructions_executed > 0` не закреплено тестом; (б) в тестах переотображения байты INVLPG в старом и новом кадре одинаковы, поэтому не различено, что подглядывание читает через кэш, а не свежим обходом; (в) контрольного прогона без сброса для `#PF` нет (вывод по устройству кэша, мутация не запускалась). Полный S4-14 открыт (в том числе поведение без инвалидации проверено лишь в том, что оно разрешено, а не как гарантия).

## S4-14i — постоянный кэш выборки между вызовами цикла набора (opt-in, не подключено; принято после ревью: P0–P2 нет, P3-1/P3-2 закрыты; коммит 1f44682, слияние 59e2e4e)

- **Файлы:** `crates/od-machine/src/fetch_translation_cache.rs` (`FetchTranslationCache::persistent`, `is_persistent`, `seal`, `revalidate`, `flush_for_host_event`, `reset`, счётчик `FetchCacheStats::host_flushes`, `invalidation()` больше не только для тестов), `crates/od-machine/src/paged_memory_adapter.rs` (цикл вынесен в `run_cpl0_set_paged_2m_ram_with_cache(.., Option<&FetchTranslationCache>, ..)`; `run_cpl0_set_paged_2m_ram_observed` стала тонкой обёрткой с кэшем на вызов, остальные точки входа не менялись), тесты в `crates/od-machine/src/linux_direct.rs` (блок S4-14i после тестов S4-14h) и в модуле тестов кэша, `docs/specs/CPU_CONTRACT.md` (раздел S4-13: абзац S4-14i). `od-isa`, `paged_pump_backend`, тестовая сессия S4-14a-g (`translation_probe_cache.rs`), `firmware_board.rs`, раннер и Linux-плата не менялись; `GuestOnly` туда не подключён.
- **Что сделано:** кэш, построенный `persistent(policy)`, принадлежит вызывающему и переживает границу вызова; границу сам ничего не сбрасывает, действуют прежние события политики (внутри цикла) и явные события хоста. Запись хоста между вызовами (загрузчик, обвязка, DMA), невидимую `committed_write`, ловит печать поколений: в конце вызова запоминаются поколения страниц всех отслеживаемых кадров таблиц, в начале следующего сверяются, любое расхождение сбрасывает все записи (`host_flushes`); запись хоста в прочие кадры, в том числе в байты кода, кэш не сбрасывает. Печать хранит и ключ контекста перевода (`TranslationKey`: `Long64ProbeConfig` CR0/CR3/CR4/EFER с CR3 после последнего `MOV CR3` и `AccessContext` цикла с привилегией по CPL состояния), в начале вызова он сравнивается с ключом входа: расхождение (хост передал другой CR3/CR0/CR4/EFER или привилегию без `MOV CR3` гостя), в том числе при пустом кэше и при несобираемой конфигурации, сбрасывает все записи (`host_flushes`), иначе под другим контекстом гость мог бы писать в таблицы прежнего (`GuestOnly` это игнорирует), печать конца вызова приняла бы новое поколение, а возврат к прежнему контексту подставил бы устаревшую запись (ревью S4-14i, P3-1). Одно и то же состояние и CR3 между вызовами дают равные ключи: записи сохраняются. `flush_for_host_event` и `reset` (записи, печать, счётчики) — явные формы. Reset машины: в `od-machine` нет операции reset платы (ACPI reset останавливает прогон, новый запуск строит новую плату), кэш плата не держит, поэтому новый сеанс получает новый кэш или `reset()`. Печать и сверка безусловны для любого кэша, переданного в `run_cpl0_set_paged_2m_ram_with_cache` (ревью S4-14i, P3-2): признак `is_persistent` на поведение не влияет, поэтому кэш `WriteWatch`, не созданный `persistent`, при повторном использовании тоже видит запись хоста в кадр таблицы (раньше терял её, вопреки «запись хоста в кадр таблицы всегда сбрасывает»). Для кэша на один вызов (путь раннера и платы, все прежние вызывающие) сверка пуста, результат и счётчики прежние, `host_flushes` на обычных путях 0.
- **Тесты (новые, все прошли):** юнит: `a_persistent_cache_is_dropped_only_by_a_write_to_a_watched_table_frame`, `a_table_frame_of_unknown_generation_is_not_trusted`, `the_host_event_flush_and_the_reset_drop_everything`; интеграционные в `linux_direct.rs`: `persistent_cache_keeps_its_entries_over_the_call_boundary` (два вызова, второй вставил 0 записей, hit вырос, остался старый код; кэш на вызов в той же программе даёт новый), `persistent_cache_changes_no_result_and_saves_the_second_walk` (обе политики, прямой код, одна вставка на оба вызова, то же конечное состояние), `persistent_cache_after_a_guest_invlpg_runs_the_new_code_in_the_next_call`, `persistent_write_watch_cache_sees_the_pte_store_of_the_previous_call`, `persistent_cache_drops_entries_for_a_host_write_to_a_watched_table_frame` (обе политики; запись в чужой кадр кэш сохраняет), `persistent_guest_only_cache_drops_the_stale_entry_on_a_host_pte_write`, `persistent_cache_reads_the_code_bytes_fresh_from_the_cached_frame`, `persistent_cache_is_dropped_by_the_explicit_host_event_and_by_reset`; после ревью P3: `a_cache_that_is_not_persistent_is_sealed_and_checked_like_any_other` (переписан, прежнее имя `..._is_neither_sealed_nor_checked` закрепляло нежелательное поведение: не-persistent кэш в повторном вызове сверяет поколения; `GuestOnly`: запись хоста в PTE даёт `host_flushes == 1` и новый код вместо старого; `WriteWatch`: запись хоста, перенаправляющая PTE на старый кадр, даёт `host_flushes == 1` и `OLD_MARKER`; без записи хоста сбросов 0), `a_one_shot_write_watch_cache_never_counts_a_host_flush` (кэш на вызов: `host_flushes == 0`, те же конечное состояние, список записей и байты кадров, что без кэша), `the_host_switching_cr3_between_calls_drops_the_cache` (кэш сброшен при смене CR3 хостом на B и обратно на A: `host_flushes` 1 и 2, запись не подставлена, `NEW_MARKER`), юнит `another_context_between_calls_drops_the_entries_and_a_return_finds_none` (сценарий ревью с генерациями, обе политики), `the_context_check_compares_every_part_of_the_key` (каждая из CR0/CR3/CR4/EFER, привилегия, источник и несобираемая конфигурация сбрасывают; тот же ключ — нет; пустой кэш считает сброс; ни разу не запечатанный не сравнивает).
- **Мутации (откатаны копией, файл сверен):** отключение сверки поколений в начале вызова ломает 2 теста записи хоста; безусловный сброс в начале каждого вызова ломает 7 тестов (в том числе проверку постоянства); отключение `flush_after_invlpg` ломает `persistent_cache_after_a_guest_invlpg_runs_the_new_code_in_the_next_call`.
- **Ограничения новой сверки:** сценарий ревью (гость под CR3=B пишет в таблицы A) в интеграции прямо не воспроизведён (в тесте вызов под B пустой, бюджет 0), его механика показана юнит-тестом на поколениях; сброс по ключу строже аппаратного TLB (разрешённо); печать теперь делается в конце каждого вызова для любого кэша (несколько чтений поколения).
- **Не закрыто:** постоянный кэш не подключён к `FirmwareBoard`, раннеру и Linux-плате (NVMe-сервис между квантами пишет через `write_bytes` с пустым обратным вызовом, то есть там нужна именно сверка поколений); доставленный `#PF` и смена CR0/CR4 через границу вызова отдельными тестами не проверены (механизм тот же, что у `INVLPG`); сверка по страницам, а не по записям (консервативно); нет кэша данных, постраничного `INVLPG`, PCID, SMP. Независимое ревью не проведено.

## S4-13b — наблюдаемость кэша выборки и обходов страниц на реальном пути Linux (только измерение; ревью Claude Opus, high: принято с ограничениями; P2 закрыт)

- **Зачем.** Единственный замер на настоящем ядре (коммит 69751e9, 200M инструкций, 554 s) покрывает только декомпрессор; фаза ядра (после ~1,14·10⁹ retired) ни разу не измерялась ни по hit/miss, ни по обходам. Поведение, вывод по умолчанию и `LinuxStopRecord` не менялись.
- **Файлы:** `crates/od-machine/src/fetch_cache_report.rs` (новый: `LinuxWalkCounts`, `LinuxFetchCachePhase`, `LinuxFetchCacheReport`, `FetchCacheAccumulator`, `WalkCounter`, `AddAssign`/`Sub` для `FetchCacheStats`), `fetch_translation_cache.rs` (`FetchCacheStats` стала `pub` с `pub` полями, остальное прежнее), `paged_memory_adapter.rs` (счётчик обходов в `access_with` адаптера инструкции; поля `walk_counts`, `c5_at_call_start`, `c5_split` в `PagedCpl0SetReport`), `linux_checkpoints.rs` (`CheckpointRecorder::is_recorded`), `firmware_board.rs` (`enable_fetch_cache_report`, `take_fetch_cache_report`, накопление по вызовам цикла `run_linux_boot_cpl0_set_with_post_iret_index`), `lib.rs`, `tools/runner/src/linux_direct.rs` (разбор `OD_NO_FETCH_CACHE`, `OD_FETCH_CACHE_STATS`, sidecar). `od-isa`, `paged_pump_backend`, `translation_probe_cache.rs`, постоянный кэш S4-14i и `GuestOnly` к раннеру и плате не подключались.
- **Что считается.** (1) `FetchCacheStats` каждого вызова цикла набора (кэш строится на вызов, то есть на квант) суммируются по прогону: hits, misses, inserts и все виды сбросов. Их lookups включают подглядывания цикла (CPUID, `IRETQ`, `INVLPG`, регистратор checkpoints). (2) Обходы на инструкцию: tracked-обходы адаптера инструкции по назначению: `fetch` (байт выборки, не обслуженный кэшем), `data_read` (чтения данных и 10-байтовое чтение псевдодескриптора), `data_write`. Один обход = один вызов tracked-ходока, успешный или нет. НЕ считаются: read-only обходы подглядываний и второй read-only обход заполнения кэша после промаха. Доставка исключений `#PF` и `#GP` (IDT, GDT, TSS, кадры) СЧИТАЕТСЯ как `data_read`/`data_write`: её обращения делает адаптер инструкции (ревью S4-13b, P2; инвариант «data с кэшем == без кэша» от этого не меняется, доставка детерминирована). Доставка внешних IRQ не считается (идёт через адаптер без счётчиков). Счётчик обходов работает всегда (одна `Cell` на обход), без кэша `fetch` считает обход каждого байта.
- **Правило фаз.** Фаза 1 — до границы, на которой записан checkpoint C5 (первая выборка по `e_entry`, `startup_64`), фаза 2 — от неё. Разрез делается на границе инструкции внутри вызова, записавшего C5 (счётчики снимаются сразу после `recorder.boundary`), поэтому он точен до инструкции, а не до кванта; подглядывания самой границы C5 относятся к фазе 1. Нужен регистратор checkpoints (`--checkpoint-table`); без него разбиения нет: всё в `before_c5`, `phase_rule` = `none_checkpoints_disabled`, `c5_reached` = false. Вызовы (`calls`) считаются в фазе, где начались; вызов, записавший C5, — в фазе 1.
- **Вывод (opt-in).** Переменная окружения раннера `OD_FETCH_CACHE_STATS=<путь>` (только с `--run --set-cpl0`): отдельный sidecar-файл JSON (`create_new`, существующий файл или совпадение с `--out` — отказ `diagnostic_sidecar_reservation_failed` до запуска). Без переменной ничего не меняется: ни файла, ни ключей отчёта, ни вызовов; `LinuxStopRecord` и `LinuxSetRunReport` без новых полей, отчёт раннера побайтно прежний. С переменной в отчёт добавляется только ключ `fetch_cache_stats_sidecar` `{path, write_error}` (как у других sidecar). Формат sidecar (`schema_version` 1): `fetch_cache_enabled` (кэш включён; прогон с `OD_NO_FETCH_CACHE` отличим), `run_completed`, `run_wall_seconds`, `phase_rule`, `c5_reached`, `notes`, и три блока `total` / `before_c5` / `after_c5`: `calls`, `retired`, `walks{fetch,data_read,data_write}`, `walks_per_instruction{...}`, `cache{hits,misses,inserts,write_flushes,context_flushes,host_flushes,invlpg_flushes,control_flushes,fault_flushes,ignored_table_writes,lookups_per_instruction,hit_rate}` (`null` при делении на 0). При `run_failed` sidecar пишется с тем, что накоплено (`run_completed` false). Без `--set-cpl0` отчёта цикла нет: запись пустого sidecar не оставляется, причина в `write_error`.
- **`OD_NO_FETCH_CACHE`.** Разбор вынесен в `fetch_cache_enabled(Option<&OsStr>)`: не задана, пустая или `0` — кэш включён; любое другое значение — выключен. **Единственное изменение поведения:** раньше выключало любое заданное значение, включая пустое и `0`; теперь `0` и пустое значение оставляют кэш. Документация и скрипты репозитория используют только `=1`.
- **Тесты (прошли).** `fetch_cache_report::tests` (5): сумма `FetchCacheStats` по всем десяти полям, разрез вызова с C5 и фазы, дающие в сумме total, прогон без кэша, счётчик обходов по видам; `linux_direct::tests` (2 новых + изменённая `assert_cache_is_invisible`): отчёт не меняет `Debug` прогона с кэшем и без, разрез по C5 на плате (4 retired до границы, 1 после), data-обходы равны с кэшем и без, fetch-обходов с кэшем меньше, без регистратора разбиения нет; в `assert_cache_is_invisible` счётчики обходов вынесены из сравнения отчётов (data равны, fetch с кэшем не больше); runner (6): `OD_NO_FETCH_CACHE` для unset/пусто/`0`/`1`/`true`/`00`, путь sidecar, JSON sidecar (флаги, обходы на инструкцию, `null` для пустой фазы), сквозной прогон с opt-in на синтетическом ядре (отчёт равен отчёту без opt-in во всех ключах, кроме `fetch_cache_stats_sidecar` и путей входов; sidecar с кэшем и с `OD_NO_FETCH_CACHE=1`), отказ при существующем файле, отсутствие sidecar без `--set-cpl0`.
- **Прогон на настоящем ядре** (release, macOS arm64, 128 MiB, nonce `s4-38l4a`, входы S4-39j, `--set-cpl0 --cpuid-profile stage4a-apic --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json`, бюджет 2,33·10⁹, `OD_FETCH_CACHE_STATS` задана; кэш включён): стоп `user_entry` (без `--user-mode`), retired 2 321 128 949, квантов 1 308 308, **wall 977,8 s** (user 975,5 s), **maximum RSS 235 388 928 B (224,5 MiB)**. Один прогон, повторов нет, разброса времени не измерено.

  | | фаза 1 (до C5) | фаза 2 (от C5) | всего |
  | --- | ---: | ---: | ---: |
  | retired | 1 137 474 211 | 1 183 654 738 | 2 321 128 949 |
  | вызовов (квантов) | 10 131 | 1 298 177 | 1 308 308 |
  | инструкций на вызов (среднее) | 112 277 | 912 | 1 774 |
  | cache hits | 8 947 548 441 | 7 354 053 315 | 16 301 601 756 |
  | cache misses | 210 707 | 38 292 520 | 38 503 227 |
  | hit rate | 0,99998 | 0,99482 | 0,99764 |
  | inserts | 40 869 | 11 982 241 | 12 023 110 |
  | lookups на инструкцию | 7,87 | 6,25 | 7,04 |
  | обходы fetch (на инстр.) | 40 869 (3,6·10⁻⁵) | 11 982 241 (0,0101) | 12 023 110 (0,0052) |
  | обходы data_read (на инстр.) | 286 498 155 (0,252) | 227 552 390 (0,192) | 514 050 545 (0,221) |
  | обходы data_write (на инстр.) | 164 130 745 (0,144) | 143 953 591 (0,122) | 308 084 336 (0,133) |
  | write_flushes / context_flushes | 45 / 1 | 619 / 26 596 | 664 / 26 597 |

  Граница разреза совпала с записью C5 прогона: retired 1 137 474 211 и квант 10 131 (как в `s4-38l8e-sci-pin9.json`). `host_flushes`, `invlpg_flushes`, `control_flushes`, `fault_flushes` равны нулю везде (путь `WriteWatch`).
- **Выводы по измеренному.** (а) На фазе ядра кэш выборки по-прежнему отрабатывает 99,5 % lookups, но промахов в 180 раз больше, чем в фазе 1: фаза 2 нарезана на кванты по ~912 инструкций (цикл останавливается на чтении RDTSC, тиках устройств и дедлайнах источника прерываний; причина нарезки по видам не разбиралась), кэш строится заново на каждый вызов и каждый раз холодный (на вызов в среднем 9,2 вставки). (б) Обходы fetch уже малы (0,005 на инструкцию, 0,010 в фазе 2); обходы данных в 68 раз больше (0,354 на инструкцию, 822 134 881 против 12 023 110): остаточная стоимость обходов в прогоне лежит в данных, а не в выборке. Это не доказывает, что обходы данных — узкое место по времени: времени по фазам и по видам обходов не измерено. (в) Сравнение с постоянным кэшем S4-14i по этим данным: он убрал бы большую часть 12 млн вставок/обходов fetch в фазе 2 (оценка по счётчикам, не замер); к плате он не подключён, прирост не измерялся.
- **Контроль «ничего не изменилось».** Короткий прогон 20·10⁶ (то же ядро, release): отчёт сборки до правки (`HEAD`) и после, без opt-in, побайтно равны (sha256 `6c94b67b…dd7a`). Полный прогон 2,33·10⁹: отчёт сборки до правки (`HEAD`, без opt-in; wall 1014,8 s, RSS 235 323 392 B) и отчёт после правки с opt-in равны как JSON за вычетом ключа `fetch_cache_stats_sidecar` (стоп `user_entry`, retired 2 321 128 949); разница времени (977,8 s против 1014,8 s) — единичные прогоны, замера накладных расходов счётчика обходов нет. Побайтного сравнения полного прогона после правки БЕЗ opt-in не делалось (покрыто 20·10⁶ и сквозным тестом раннера). Отчёт `s4-38l8e-sci-pin9.json` (бюджет 2,6·10⁹, retired 2 321 470 691, C11–C16 на 0,3–0,6·10⁶ позже) отличается от обоих прогонов на текущем `main`: расхождение уже есть в сборке до правки, этим срезом не вызвано и не разбиралось. Сравнение 20·10⁶ с кэшем и с `OD_NO_FETCH_CACHE=1` по sidecar: data-обходы равны (4 274 318 чтений, 7 061 755 записей), fetch-обходов 2 404 с кэшем и 72 073 288 без него (3,60 на инструкцию), wall 8,8 s против 16,3 s (замер параллельно с другим прогоном, ориентировочно).
- **Не измерено / ограничения.** Время по фазам и по видам обходов; прогон без кэша на 2,33·10⁹ (только 20·10⁶); повторы и разброс; обходы подглядываний, заполнения кэша и доставки исключений (в `fetch` не входят); кэш данных; `--user-mode` и профиль `stage4a` (прогон — `stage4a-apic` без user mode, стоп на первом `IRETQ` в CPL3 как в `s4-38l8e`); числа `retired` и номера вызовов зависят от того, что регистратор checkpoints включён (его подглядывание ходит через кэш и двигает hits/misses). Sidecar данных прогона в репозиторий не добавлялся.
- **Не закрыто:** ревью принято с ограничениями (P2 про доставку `#PF`/`#GP` в счётчиках данных закрыт правкой текстов; P3 — тексты и тесты sidecar). Решение о подключении постоянного кэша к плате принято в S4-13f: не подключать в S4-13 (см. раздел S4-13e).

## S4-13c — теневые кэши: sweep ёмкости и политики кэша выборки на реальном пути Linux (только диагностика; S4-13 открыт; ревью с исправлениями: принято; данные пересчитаны после исправления P3-1)

- **Зачем.** S4-13b показал, что в фазе ядра кэш выборки (4 слота, FIFO, на вызов, тег 4 KiB, `WriteWatch`) даёт 99,5 % попаданий, но строится заново на каждый квант (~912 инструкций, ~9,2 вставки на квант). Срез отвечает по данным, что дали бы другая ёмкость, замена, гранулярность тега и жизненный цикл. Поведение по умолчанию, `LinuxStopRecord`, отчёт платы и счётчики настоящего кэша не менялись; `GuestOnly` и постоянный кэш S4-14i к раннеру и плате не подключались (теневые кэши только моделируют их политику).
- **Файлы:** `crates/od-machine/src/fetch_cache_sweep.rs` (новый: теневые кэши, сетка, отчёт, 14 юнит-тестов), `fetch_translation_cache.rs` (необязательное поле `sweep: Option<Rc<FetchCacheSweep>>`, `with_sweep`, пересылка событий; `lookup_fetch` отличает выборку от подглядывания; `TranslationKey` с `pub(crate)` полями; без sweep поведение и счётчики прежние), `paged_memory_adapter.rs` (`run_cpl0_set_paged_2m_ram_observed_with_sweep`; сообщения теням о `#PF`, смене CR0/CR4, отметка C5; подглядывание `INVLPG` мимо кэша только при sweep), `firmware_board.rs` (`enable_fetch_cache_sweep`, `take_fetch_cache_sweep`), `lib.rs`, `tools/runner/src/linux_direct.rs` (`OD_FETCH_CACHE_SWEEP`, ключ `fetch_cache_sweep` в sidecar S4-13b, 2 теста раннера), 2 теста платы и цикла в `linux_direct.rs`. `od-isa`, `paged_pump_backend`, `translation_probe_cache.rs` не тронуты.
- **Устройство.** Настоящий кэш пересылает подключённому `FetchCacheSweep` каждое событие: lookup (с признаком «выборка/подглядывание» и таблицами записи, если попал), insert, `note_write`, явные сбросы, границу вызова (`begin_call`, `revalidate`, `seal`, `flush_for_host_event`, `reset`). Цикл дополнительно сообщает о `INVLPG` (подглядывание RIP мимо кэша: счётчики кэша не двигаются), доставленном `#PF` и смене CR0/CR4; на них реагируют только теневые `GuestOnly`. Каждая тень сама решает hit/miss по своему состоянию; на промахе выборки она «ходит и вставляет»: если настоящий кэш попал, берёт таблицы его записи, если тоже промахнулся, получает его insert; подглядывание ничего не вставляет. Выход теней не читается ничем в цикле; он идёт только в отдельный ключ sidecar.
- **Сетка (112 теней).** Ёмкость {1, 2, 4, 8, 16, 32, 64} × замена {FIFO, LRU} × тег {`4k` — страница 4 KiB, `2m` — запись 2 MiB-листа покрывает весь лист; 4 KiB-лист остаётся 4 KiB} × жизненный цикл {`per_call` — очищается в начале каждого вызова, как настоящий кэш платы; `persistent` — живёт между вызовами по правилам S4-14i: смена контекста трансляции или запись в наблюдаемый кадр таблицы вне цикла сбрасывает} × инвалидация {`write_watch` — запись в таблицу записи сбрасывает всё (S4-13); `guest_only` — запись только считается, сбрасывают `INVLPG`, `#PF`, CR0/CR4, `MOV CR3`}. Ключ в JSON: `cap<N>-<fifo|lru>-<4k|2m>-<per_call|persistent>-<write_watch|guest_only>`. `guest_only` — гипотеза: тень продолжает обслуживать трансляции, которым прогон не доверял бы; её hits — то, что видел бы TLB-подобный кэш, а не результат прогона. Для `persistent` записи хоста между вызовами видны только через поколения страниц.
- **Самопроверка (обязательна, пройдена).** Тень «4 / FIFO / 4k / per_call / write_watch» совпала с настоящим кэшем по ВСЕМ десяти счётчикам (hits, misses, inserts, write/context/host/invlpg/control/fault flushes, ignored_table_writes): (а) юнит-тесты на детерминированной синтетике (4 зерна, 300 вызовов, смесь 4 KiB- и 2 MiB-обходов, записи в таблицы, сбросы, подглядывания, неудачные обходы); (б) тень `guest_only` против настоящего `GuestOnly`-кэша с `INVLPG`/`#PF`/CR0/CR4; (в) постоянные тени против настоящего `persistent` кэша со сменой контекста и записями хоста между вызовами; (г) на плате (синтетическое ядро): total, фаза 1 и фаза 2 равны отчёту платы; (д) на настоящем ядре: `self_check` в sidecar — `total_equals_real`, `phase1_equals_real`, `phase2_equals_real` все `true`; настоящие счётчики этого прогона (hits 16 301 601 756, misses 38 503 227, inserts 12 023 110, write_flushes 664, context_flushes 26 597) равны данным S4-13b по всем фазам, обходы fetch/data_read/data_write тоже. Расхождений нет, подгонки нет. Отдельный тест сверяет «один подсчёт на все тени» (оптимизация подряд идущих lookup одной страницы) с поштучным подсчётом на всех 112 тенях.
- **Пересчёт после исправления P3-1 (тот же вход, release, те же флаги и env, один прогон).** Сборка `od-runner` после исправлений ревью. `self_check`: `total_equals_real`, `phase1_equals_real`, `phase2_equals_real` все `true`, сравниваются все десять счётчиков. Счётчики настоящего кэша равны прежним: hits 16 301 601 756, misses 38 503 227, inserts 12 023 110, write_flushes 664, context_flushes 26 597 (фаза 1 и фаза 2 тоже равны S4-13b). События: lookups 16 340 104 983, оптом 16 105 599 776, записей 308 088 904, к теням дошло 13 367, кадров таблиц 18, контекстов 9 (как в первом прогоне). Счётчики всех 112 теней (десять полей × фаза 1, фаза 2, total) равны первому прогону: случай P3-1 (все тени попали, настоящий кэш промахнулся) на этом прогоне не меняет ни одного счётчика; в файле данных изменились только значения `run.wall_seconds` и `run.max_rss_bytes` и список `notes` (добавлены два пояснения). Таблица и выводы ниже поэтому не изменились. Отчёт раннера этого прогона равен отчёту S4-13b (`base-2330m.json`) как JSON за вычетом ключей `fetch_cache_stats_sidecar` и `inputs`.
- **Время и память (после исправления P3-1).** Wall 1383,06 s (user 1360,85 s, sys 4,57 s), maximum RSS 234 962 944 B (224,1 MiB). Без теней в S4-13b было 977,8 s: sweep дороже примерно на 41 % (одиночные прогоны, чужих прогонов раннера во время этого не было). Разброс не измерялся.
- **Влияния на прогон нет.** Тесты: отчёт платы (`Debug`) с sweep и без равен побайтно, счётчики настоящего кэша равны (плата и цикл с `INVLPG`); runner: evidence с `OD_FETCH_CACHE_SWEEP` равно evidence без него, флаг без `OD_FETCH_CACHE_STATS` ничего не пишет. Короткий прогон 20·10⁶ на ядре: отчёт с sweep и без равен после снятия ключа sidecar и `inputs`. Полный прогон: отчёт раннера (за вычетом `fetch_cache_stats_sidecar` и `inputs`) равен отчёту прогона S4-13b на том же входе (стоп `user_entry`, retired 2 321 128 949).
- **Прогон на настоящем ядре** (release, macOS arm64, 128 MiB, nonce `s4-38l4a`, `--set-cpl0 --cpuid-profile stage4a-apic --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json`, бюджет 2,33·10⁹, `OD_FETCH_CACHE_STATS` + `OD_FETCH_CACHE_SWEEP=1`): retired 2 321 128 949 (фаза 1 — 1 137 474 211, фаза 2 — 1 183 654 738), 1 308 308 вызовов; первый прогон (до исправления P3-1): wall 1354,7 s (user 1348,5 s; без теней был 977,8 s: параллельно шёл чужой прогон раннера), maximum RSS 236 437 504 B (225,5 MiB). Данные (пересчитаны на коде после исправления P3-1, см. строку «Пересчёт» ниже): `docs/reports/data/stage4/s4-13c-sweep-2026-11-16.json` (схема: ключ конфигурации → `phase1_before_c5`, `phase2_after_c5`, `total`, в каждом десять счётчиков и `hit_rate`; плюс счётчики настоящего кэша, `self_check`, события sweep). Сырой sidecar в репозиторий не добавлялся. События: lookups 16 340 104 983, из них 16 105 599 776 посчитаны оптом; 308 088 904 записи, к теням дошло 13 367; наблюдаемых кадров таблиц 18, различных контекстов трансляции 9.
- **Результаты (misses в фазе 2, после C5; база — 4 / FIFO / 4k / per_call / write_watch = 38 292 520, total 38 503 227).**

  | жизненный цикл, тег (write_watch) | замена | cap 1 | 2 | 4 | 8 | 16 | 32 | 64 |
  | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
  | per_call, 4k | FIFO | 118,1 M | 87,8 M | **38,3 M (база)** | 20,9 M | 11,1 M | 8,6 M | 8,2 M |
  | per_call, 4k | LRU | 118,1 M | 81,9 M | 32,3 M | 17,2 M | 10,4 M | 8,5 M | 8,2 M |
  | per_call, 2m | FIFO | 77,3 M | 35,6 M | 7,6 M | 4,92 M | 4,89 M | 4,89 M | 4,89 M |
  | per_call, 2m | LRU | 77,3 M | 32,2 M | 6,5 M | 4,90 M | 4,89 M | 4,89 M | 4,89 M |
  | persistent, 4k | FIFO | 114,7 M | 84,3 M | 34,7 M | 16,4 M | 5,90 M | 3,26 M | 2,51 M |
  | persistent, 4k | LRU | 114,7 M | 78,3 M | 28,3 M | 12,6 M | 5,20 M | 3,09 M | 2,40 M |
  | persistent, 2m | FIFO | 73,9 M | 32,1 M | 3,73 M | 616,7 k | 555,5 k | 554,8 k | 554,7 k |
  | persistent, 2m | LRU | 73,9 M | 28,5 M | 2,56 M | 590,8 k | 555,2 k | 554,7 k | 554,7 k |

  Лучшая по фазе 2 и по total: **64 / FIFO (= LRU) / 2m / persistent / write_watch**: фаза 2 — 554 736 misses (hit rate 0,999925; база 0,994820; в 69 раз меньше промахов), total — 554 995 (0,999966; база 0,997644), inserts в фазе 2 — 122 905 (база 11 982 241). Ёмкость 8 с LRU на том же теге и цикле даёт 590 798 (в 65 раз меньше базы); дальше 16 кривая плоская (555 k). Монотонность в этих данных: в каждой из 16 серий по ёмкости (жизненный цикл × тег × замена × инвалидация) misses фазы 2 не растут при росте ёмкости с 1 до 64, и LRU не хуже FIFO ни в одной из 56 пар при той же остальной конфигурации; это наблюдение по одному прогону, а не свойство политик. Среди `per_call`-теней лучшая — 64 / LRU / 4k (8,23 M, ×4,7) и любая ёмкость ≥ 8 с 2m (≈ 4,9 M, ×7,8): сам по себе `per_call` ограничен холодным стартом каждого кванта (~1,3 M вызовов). `guest_only` дополнительно лучшего не даёт по misses в фазе 2: лучшая 64 / 2m / persistent — 1 108 571 (хуже `write_watch`: из-за ограничения «`INVLPG` сбрасывает весь кэш» 70 207 `INVLPG`-сбросов сбрасывают кэш чаще, чем 987 сбросов по записи в таблицы (`write_flushes`)), то есть учёт записи в таблицу как сброса в этих данных дешёв: `write_watch` write_flushes 619 (база, фаза 2) и 987 (лучшая тень) против 0 и 2 008 игнорированных записей у `guest_only`. `host_flushes` равны нулю везде (между вызовами контекст и таблицы не менялись), `control_flushes` — 6, `fault_flushes` — 30 за прогон у `guest_only`.
- **Что НЕ измерено / ограничения.** (1) Время: по фазам и по конфигурациям; число misses — не время, сколько стоит промах/вставка/поиск в 64 слотах и что даст `persistent` на самом деле, не измерено (это S4-13e). Время sweep-прогона не сравнимо с прогоном без него. (2) Повторные прогоны и разброс (один прогон, но счётчики детерминированы). (3) Тени получают трассу lookup прогона и результат обхода из настоящего кэша; это эквивалентно независимому кэшу только при `WriteWatch`-прогоне (тень, промахнувшаяся там, где настоящий кэш попал, подставляет таблицы его записи); при промахе выборки тень сама решает hit/miss и получает вставку, если промахнулась; P3-1 (исправлен) касался случая, когда все тени попали, а настоящий кэш промахнулся (вставку тогда ошибочно получали все тени). Для `guest_only` и `persistent` — модель, а не исполнение: настоящий `GuestOnly` мог бы вернуть устаревшую трансляцию и изменить трассу, `persistent` в плате не работает (не подключён). (4) `INVLPG` для `guest_only` определяется подглядыванием RIP мимо кэша; сброс всего кэша, а не страницы операнда. Настоящий `GuestOnly` ищет `INVLPG` подглядыванием через кэш (Peek-lookup добавляется в его hits/misses), тень `guest_only` под `WriteWatch`-прогоном — мимо кэша; поэтому счётчики hits/misses `guest_only`-тени на плате могут отличаться от настоящего `GuestOnly`; набор сбрасывающих событий (`INVLPG`, `#PF`, CR0/CR4, `MOV CR3`) совпадает. (5) Записи хоста между вызовами (DMA NVMe и т. п.) в `persistent` видны только по поколениям страниц таблиц, наблюдавшихся когда-либо (надмножество; на этом прогоне сбросов `host_flushes` нет). (6) Гранулярность `2m` предполагает, что запись листа 2 MiB безопасно покрывает лист; разрешения те же для всего листа, но проверка через настоящий кэш с таким тегом не делалась (S4-13d). (7) Выбор конфигурации, подключение к плате и решение по S4-13 не принимались.
- **Не закрыто:** чекбокс S4-13 не отмечен; выбор конфигурации и подключение к плате (S4-13d, S4-13e).

## S4-13d — тег 2 MiB в настоящем кэше выборки и расширенный дифференциальный гейт (ревью Claude Opus, high: принято с ограничениями; P2 закрыт тестом congruent-4k)

- **Зачем.** В S4-13c тег `2m` проверялся только на теневых кэшах (модель), настоящий кэш тегировал только 4 KiB; дифференциальный гейт на ядре покрывал лишь префикс декомпрессора (20·10⁶), а фаза ядра и ряд синтетических случаев (сбросы A-бита на верхних уровнях, D-бит в таблице кэшированного кода, CR3 A→B→A с правкой таблиц, remap/NX 2 MiB-листа) не проверялись. Поведение по умолчанию (тег 4 KiB, `WriteWatch`), `LinuxStopRecord`, отчёты платы и раннера не менялись; `GuestOnly` и постоянный кэш S4-14i к раннеру и плате не подключались.
- **Файлы.** `crates/od-machine/src/physical_memory.rs` (`RamFrame` получил признак `large`, `PhysicalMemory::ram_leaf_2m`, `read_ram_frame_u8` берёт младшие 12 бит адреса запроса для 4 KiB и младшие 21 бит для 2 MiB-пролёта), `fetch_translation_cache.rs` (`FetchCacheTag { Page4K (по умолчанию), Leaf2M }`, `with_tag`, `tag`, поле `shift` записи: 12 или 21; ключ записи — адрес `>> shift`; замена записи «на месте» учитывает `shift`), `paged_memory_adapter.rs` (`fetch_through_cache`: при теге `Leaf2M` и обходе, закончившемся 2 MiB-листом (три таблицы), и пролёте 2 MiB, целиком лежащем в plain RAM, запись создаётся для всего листа; иначе — 4 KiB-запись; `run_cpl0_set_paged_2m_ram_observed_with_sweep` получил параметр `tag`, остальные вызывающие передают `Page4K`), `firmware_board.rs` (`enable_fetch_cache_leaf_2m_tag`, `ram_sha256`: диагностика конца прогона), `fetch_cache_report.rs` (поле `leaf_2m_tag` отчёта), `tools/runner/src/linux_direct.rs` (`OD_FETCH_CACHE_TAG=2m` — только точное значение `2m`; в sidecar S4-13b ключи `fetch_cache_tag` и `final_ram_sha256`; 2 теста раннера), `linux_direct.rs` (синтетический гейт). `od-isa`, `paged_pump_backend`, `translation_probe_cache.rs` не тронуты. Отчёт раннера и плата без `OD_FETCH_CACHE_TAG` побайтно прежние (ключи добавлены только в sidecar, который существует лишь с `OD_FETCH_CACHE_STATS`).
- **Устройство тега 2 MiB.** Запись держит базу 2 MiB-пролёта (обработчик `RamFrame` с `large = true`) и тег = линейный адрес >> 21 вместо >> 12. Смещение внутри листа берётся из адреса КАЖДОГО запроса (`read_ram_frame_u8(frame, address)` маскирует `0x1f_ffff`), не из записи. База вычисляется как `physical − (address & 0x1f_ffff)` и должна быть 2 MiB-выровненной и целиком в одном регионе Ram с backing (иначе — 4 KiB-запись, как без тега). Наблюдаются три таблицы (PML4, PDPT, PD) — запись в любую сбрасывает кэш (`WriteWatch`) так же, как для 4 KiB; ключ по-прежнему содержит CR0/CR3/CR4/EFER и контекст (режим, привилегия), сбросы (`MOV CR3`, `INVLPG`, `#PF`, CR0/CR4, события хоста, проверка на границе вызова) очищают 2 MiB-записи теми же функциями. Права и A/D не кэшируются отдельно: запись создаётся только после реального успешного обхода (A-биты всех уровней уже стоят), неудачи не кэшируются; права листа одинаковы для всех его страниц, а D-бит выборку не касается.
- **Тесты (все прошли).** Блок кэша: `cargo test -p od-machine --offline -- --test-threads=1 cache` — 71 passed, 1 ignored (реальное ядро, как раньше), 0 failed.
  - Юнит-тесты `fetch_translation_cache` (4 новых): `the_default_tag_is_4k_and_a_2m_handle_needs_a_wholly_ram_aligned_span` (тег по умолчанию 4 KiB; хэндл 2 MiB только для выровненного пролёта целиком в RAM: не выровненная база, пролёт, выходящий за RAM, ROM и дыра — без хэндла); `a_2m_entry_serves_every_page_of_its_leaf_with_the_offset_of_the_request` (запись, созданная запросом внутри листа, даёт hit для смещений 0, 1, 0x1000, 0x1234, 0x1f_f000, 0x1f_ffff, и байт читается с физического `база + смещение запроса`; соседние листы, другой CR3 и другая привилегия — промах); `a_4k_handle_is_a_4k_entry_in_either_tag_and_entries_of_both_sizes_coexist` (4 KiB-хэндл — 4 KiB-запись при любом теге; записи обоих размеров сосуществуют, ключ заменяется на месте, тег по умолчанию не расширяет); `a_2m_entry_is_dropped_by_the_events_that_drop_a_4k_entry` (запись в любую из трёх таблиц, включая PD-кадр, сбрасывает; запись в четвёртый, неиспользуемый слот и в сам лист — нет; `GuestOnly` считает запись и не сбрасывает; `MOV CR3`, `INVLPG`, `#PF`, CR0/CR4, событие хоста, смена CR3 и поколения на границе вызова сбрасывают).
  - Синтетический гейт на плате (`linux_direct.rs`, по образцу `assert_cache_is_invisible`; `assert_modes_agree`: каждый случай гоняется без кэша, с 4 KiB-кэшем и с кэшем тега 2 MiB, и все три должны совпасть по состоянию CPU, отчёту (выход, запись `#PF`, смены контекста), всем committed writes в порядке и байтам кадров (таблицы с A/D, код, данные); обходы данных равны, обходов выборки с кэшем не больше). Каждый случай идёт на двух раскладках: страница 4 KiB за PT и страница 2 MiB-листа (PDE с PS).
    1. `cache_2m_tag_serves_every_4k_page_of_a_leaf_from_one_entry` — 640 `MOV RAX, imm` через три 4 KiB-страницы одного листа: с тегом 4 KiB 3 вставки, с тегом 2 MiB — 1; те же lookups (hits + misses равны), меньше промахов и обходов выборки; RAX = 639 как без кэша. Падает при неверном смещении/кадре (мутация d ниже).
    2. (а) `gate_2m_leaf_remap_under_write_watch_runs_the_new_leaf` — гость переписывает PDE листа, на котором исполняется, на другой лист: следующая инструкция идёт из нового листа (маркер 0x3333), по одному `write_flush` у каждого кэша, итоговый PDE равен новому. `gate_2m_leaf_nx_store_faults_like_without_the_cache` — гость ставит NX (бит 63) в PDE своего листа 32-битной записью старшей половины: следующая выборка — тот же `#PF` с кодом 0x11, линейным адресом и сохранённым RIP следующей инструкции, 3 инструкции; кэш не отдаёт старую запись. `gate_2m_leaf_unmap_store_faults_like_without_the_cache` — Present сброшен: `#PF` 0x10, неудачный обход не кэшируется.
    3. (б) `gate_cleared_accessed_bit_of_a_pde_/_pdpte_/_pml4e_is_set_again_on_the_next_fetch` (по обеим раскладкам: 4 KiB и 2 MiB-лист; для PDE 2 MiB-листа — PDE с PS и очищенным A) — гость переписывает запись соответствующего уровня пути своего кода с очищенным A-битом (та же цель); следующая выборка должна пойти в обход и снова поставить A: в RAM A установлен, последний committed write — 8 байт по адресу этой записи, один `write_flush`. Кэш, оставивший старую трансляцию, оставил бы A очищенным.
    4. (в) `gate_dirty_commit_on_the_page_of_the_cached_code_matches_the_uncached_run` и `gate_dirty_commit_for_another_page_of_the_same_table_matches_the_uncached_run` — код пишет данные в страницу (PDE листа) с D=0: обход записи коммитит D в кадр таблицы, который наблюдает запись кода (данные на той же странице; на другой странице той же PT; в другом 2 MiB-листе того же PD): один `write_flush`, повторная вставка после сброса, D-бит и данные в RAM, committed writes равны.
    5. (г) `gate_cr3_a_b_a_with_a_4k_page_runs_the_edited_tables_of_a` и `..._with_a_2m_leaf_...` — в одном прогоне `MOV CR3` на B, в B гость через окно правит запись дерева A (вариант 2), `MOV CR3` обратно на A: инструкция после второго `MOV CR3` должна прийти из правленой страницы A (маркер 0x3333), а не из записи, созданной под A до первого переключения (маркер 0x1111), и не из записи B (0x2222). Оба режима: `WriteWatch` и `GuestOnly` (TLB: запись в таблицу ничего не сбрасывает, остаётся только сброс `MOV CR3`; именно он ловит потерянный сброс на `MOV CR3`).
    6. Плата: `the_2m_tag_is_opt_in_on_the_board_and_leaves_the_run_report_unchanged` — тег выключен по умолчанию (в отчёте `leaf_2m_tag` false), `Debug` отчёта прогона равен без кэша / с кэшем 4 KiB / с тегом 2 MiB; тег без кэша ничего не меняет.
  - Раннер (`cargo test -p od-runner --offline -- fetch_cache`: 10 passed): `od_fetch_cache_tag_is_exactly_2m_or_the_4k_default`; `fetch_cache_tag_2m_changes_no_evidence_and_the_sidecar_names_it` (evidence равен для 4k / 2m / без кэша, sidecar называет тег, `final_ram_sha256` равен во всех трёх).
- **Мутации (прогон блока гейта, 25 тестов; изменения откатывались копией файла).** (a) `WriteWatch` не сбрасывает записи при записи в таблицу: падают 8 тестов (remap, NX, unmap, три сброса A, два D-теста); CR3-тесты проходят (сброс на `MOV CR3` отдельный). (b) `flush()` (`MOV CR3`) ничего не делает: падают оба CR3-теста (с первой же `GuestOnly`-итерации; под `WriteWatch` одной правки таблицы A достаточно, чтобы тест не поймал потерю сброса, поэтому тест дополнен `GuestOnly`). (d) смещение внутри 2 MiB-пролёта маскируется 12 битами: падают `cache_2m_tag_serves…` и все остальные тесты 2 MiB-раскладки (10 из 25). (e) PML4-кадр не наблюдается: падает только `gate_cleared_accessed_bit_of_a_pml4e…`.
- **Прогон на настоящем ядре** (release, macOS arm64, 14 ядер, 128 MiB, nonce `s4-38l4a`, входы S4-39j, `--set-cpl0 --cpuid-profile stage4a-apic --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json`, как в S4-13b/c; фаза ядра до `user_entry`; `OD_FETCH_CACHE_STATS` задана). Три режима: **4k** (по умолчанию), **2m** (`OD_FETCH_CACHE_TAG=2m`), **nocache** (`OD_NO_FETCH_CACHE=1`). Сравнивается отчёт раннера за вычетом `fetch_cache_stats_sidecar` и `inputs` (JSON с отсортированными ключами, sha256), хэш всей plain-RAM в конце прогона (`final_ram_sha256`, новый ключ sidecar) и счётчики обходов данных. Файл данных: `docs/reports/data/stage4/s4-13d-differential-2026-11-16.json`.

  | бюджет | режим | retired / стоп | sha256 отчёта (без sidecar/inputs) | sha256 RAM | обходы data_read / data_write | обходы fetch | wall / user (с; для 20·10⁶ только wall) | max RSS |
  | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: |
  | 20·10⁶ | 4k | 20 000 000, `budget_exhausted` | `689b887a…` | `8dad105f…` | 4 274 318 / 7 061 755 | 2 404 | 9,5 | 608 MiB |
  | 20·10⁶ | 2m | то же | `689b887a…` | `8dad105f…` | то же | 2 159 | 9,4 | 608 MiB |
  | 20·10⁶ | nocache | то же | `689b887a…` | `8dad105f…` | то же | 72 073 288 | 17,4 | 608 MiB |
  | 2,33·10⁹ | 4k | 2 321 128 949, `user_entry` | `7687f27f…` | `aa5da8bb…` | 514 050 545 / 308 084 336 | 12 023 110 | 1113,3 / 1060,4 | 550,6 MiB |
  | 2,33·10⁹ | 2m | то же | `7687f27f…` | `aa5da8bb…` | то же | 2 711 448 | 1109,2 / 1056,2 | 550,5 MiB |
  | 2,33·10⁹ | nocache | то же | `7687f27f…` | `aa5da8bb…` | то же | 8 927 242 593 | 2084,0 / 1993,1 | 549,9 MiB |

  Результат: **при каждом бюджете три режима равны побайтно** по отчёту (стоп-запись, регистры, счётчики `cr3_changes` 26 597, `exceptions_delivered` 31 и т. д.), по хэшу RAM и по обходам данных; различаются только счётчики обходов выборки (по замыслу). Счётчики настоящего кэша с тегом 4 KiB на 2,33·10⁹ равны данным S4-13b/c (hits 16 301 601 756, misses 38 503 227, inserts 12 023 110, write_flushes 664, context_flushes 26 597; фаза 1 — 10 131 вызов, фаза 2 — 1 298 177).
- **Тег 2 MiB на ядре (измерено, одиночные прогоны).** Misses всего 7 673 778 против 38 503 227 (в 5,0 раза меньше; фаза 2 — 7 612 835 против 38 292 520, что совпадает с оценкой S4-13c для `per_call / 2m / cap 4 / FIFO / write_watch` — 7,6 M), inserts и обходы выборки 2 711 448 против 12 023 110 (в 4,4 раза меньше), hits 16 332 431 205 против 16 301 601 756 (те же lookups: 16 340 104 983). Фаза 1: 10 164 обхода вместо 40 869. `write_flushes` растут: 1 032 против 664 (фаза 2: 987 против 619); причина не установлена; одна из гипотез — при SLOTS=4 2 MiB-записи реже вытесняются и чаще наблюдают кадр в момент записи. Обходы данных не изменились: 514 050 545 + 308 084 336, то есть остаток обходов — данные (0,354 на инструкцию), а не выборка (0,0012 на инструкцию с тегом 2 MiB, 0,0052 с 4 KiB, 3,85 без кэша).
- **Время (не замер скорости).** 4k 1113,3 s и 2m 1109,2 s — разница 0,4 % при трёх одновременных прогонах (4k, 2m, nocache) и параллельных сборках/тестах на той же машине; это не измерение выигрыша тега (S4-13e), он в шуме. Без кэша 2084,0 s (1,87 раза дольше 4k; на 20·10⁶ — 17,4 s против 9,5 s, 1,83 раза). Оценка из задания «~6 ч без кэша» не подтвердилась: фактически 34,7 минуты на 2,33·10⁹. RSS 550 MiB (577 323 008 B) на 342 MiB больше, чем в S4-13b (235 388 928 B): прирост — буферы хэша RAM, который вычисляет sidecar в конце прогона (в прогон без `OD_FETCH_CACHE_STATS` это не входит).
- **Фаза после входа в user mode (`--user-mode`, бюджет 3·10⁹; те же три режима, входы и `--checkpoint-table`).** Две конфигурации, три режима в каждой (отчёты и RAM равны во всех шести прогонах внутри конфигурации):

  | конфигурация | режим | retired / стоп | sha256 отчёта | sha256 RAM | обходы data_read / data_write | обходы fetch | misses | wall (с) |
  | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: |
  | `stage4a-apic --user-mode` (как S4-13b/c + user mode) | 4k | 2 322 564 102, `backend` | `ee8030b6…` | `2ff2a605…` | 514 323 788 / 308 286 454 | 12 052 783 | 38 626 605 | 1053,4 |
  | | 2m | то же | то же | то же | то же | 2 722 265 | 7 707 912 | 1052,1 |
  | | nocache | то же | то же | то же | то же | 8 931 880 171 | 0 | 1894,7 |
  | `stage4a --user-mode` (конфигурация S4-39j/S4-38L4i) | 4k | 2 999 999 998, `budget_exhausted` | `d41005e7…` | `a64e8b25…` | 651 817 346 / 400 540 770 | 51 103 787 | 228 996 469 | 1354,9 |
  | | 2m | то же | то же | то же | то же | 37 854 188 | 181 120 441 | 1356,8 |
  | | nocache | то же | то же | то же | то же | 11 295 795 484 | 0 | 2541,0 |

  Прогон `stage4a-apic --user-mode` на текущем `main` кончается не бюджетом, а `exit_kind: backend` на retired 2 322 564 102 (RIP `0xffffffff813fb122`, запись в MMIO по физическому `0xFEE0_0320` (область LAPIC): «MMIO device 1 write failed: UnsupportedConfiguration»). Это поведение платы с профилем `stage4a-apic` после user_entry, не вызванное этим срезом (идентично во всех трёх режимах, включая без кэша; сравнение с чистым `HEAD` без правок на этой конфигурации не делалось). Конфигурация `stage4a --user-mode` доходит до бюджета (retired 2 999 999 998, как в S4-38L4i). В ней тег 2 MiB даёт слабее выигрыш (fetch-обходов 37,9 M против 51,1 M, −26 %), вероятно потому, что после входа в user mode выборка идёт и со страниц процесса `/init` (4 KiB-листья, где тег 2 MiB равен 4 KiB); причина по данным не разбиралась.
- **Не делалось / ограничения.** (0) Все прогоны одного бюджета шли параллельно на одной машине; полные прогоны выполнены по одному разу, повторов нет. (1) Постоянный кэш S4-14i и `GuestOnly` на ядре не гонялись (не подключены); `GuestOnly` проверен только на синтетическом CR3-случае. (2) Права CPL3/U/S и WP-случаи 2 MiB-листа: ключ содержит привилегию и режим (юнит-тест), но отдельного синтетического случая «листовая запись без U/S, выборка из CPL3» нет; фаза CPL3 на ядре покрыта только прогонами `--user-mode` выше (вероятно, на них тег 2 MiB не действует: страницы user mode — 4 KiB по структуре раскладки; не проверено). (3) Время повторами не мерилось (S4-13e). (4) Тег 2 MiB включён только по `OD_FETCH_CACHE_TAG=2m` в раннере и `enable_fetch_cache_leaf_2m_tag` на плате; решение об умолчании не принималось. (5) Теневой sweep вместе с тегом 2 MiB не гонялся: теневые кэши получают те же события, но самопроверка S4-13c определена для настоящего кэша с тегом 4 KiB. (6) Хэш RAM — только в конце прогона: промежуточные состояния не сравнивались (сравнивались стоп-запись и счётчики обходов). (7) Независимое ревью проведено (Claude Opus, high): принято с ограничениями; замечания P2 и P3 учтены, как записано в этом разделе. (8) `ram_sha256` копирует всю RAM в один буфер (пик 2–3× размера RAM); потоковый вариант — отдельная задача. Нечитаемый регион RAM теперь не пропускается молча: хэш в sidecar — `null`, причина в `final_ram_sha256_error`. (9) По ревью добавлено: синтетические случаи `gate_2m_congruent_4k_page_*` (4 KiB-страница с кадром, конгруэнтным адресу по модулю 2 MiB, остаётся 4 KiB-записью; мутация снятия условия `tables[3].is_none()` роняет оба); `OD_FETCH_CACHE_TAG=2m` без `OD_FETCH_CACHE_STATS` не применяется, раннер пишет предупреждение в stderr (как sweep, тег едет на sidecar); тест `the_2m_tag_is_opt_in…` проверяет только opt-in и равенство `Debug`-текста отчёта, неверное смещение 2 MiB-записи он не ловит.
- **Не закрыто:** чекбокс S4-13 не отмечен; тайминг и правило решения (S4-13e), применение и закрытие (S4-13f).

## S4-13e — замер времени кэша выборки на ядре и правило решения (только измерение; gate-ревью Claude Opus, high, 2026-11-16: принято с ограничениями; вердикт — умолчание 4 KiB)

- **Зачем.** S4-13d показал, что тег 2 MiB снижает обходы выборки в 4,4 раза и промахи в 5,0 раза, но единственная пара прогонов (4k 1113,3 s, 2m 1109,2 s) шла параллельно с другими прогонами и сборками, поэтому выигрыш по времени не доказан. Решение о смене умолчания (4 KiB, 4 слота FIFO, кэш на вызов) принимается по времени, а не по числу промахов. Код и умолчание не менялись.
- **Методика.** Release `od-runner`, собранный из чистого `main` `1a63de3` (sha256 бинарника `421d8ca2…c2e1`, копия вне дерева, чтобы сборки не меняли файл), конфигурация как в S4-13b/c/d: `linux-direct --nonce s4-38l4a --ram-mib 128 --run --budget 2330000000 --set-cpl0 --cpuid-profile stage4a-apic --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json`, входы S4-39j (vmlinuz 7.0.0-38, `initramfs.cpio`, ACPI-блоб, те же `--expected-*`), стоп `user_entry`. Режимы: **4k** (по умолчанию), **2m** (`OD_FETCH_CACHE_TAG=2m`), **nocache** (`OD_NO_FETCH_CACHE=1`). `OD_FETCH_CACHE_STATS` задана во ВСЕХ режимах одинаково (без неё тег 2 MiB не применяется; sidecar даёт хэш RAM для проверки равенства; его накладные расходы — счётчики и хэш RAM в конце прогона — одинаковы у трёх режимов, абсолютное время «настоящего умолчания без sidecar» не измерялось). Прогоны строго последовательные (один процесс в каждый момент; во время замеров не запускались сборки и тесты), по кругу 4k → 2m → nocache ×3, затем по одному дополнительному 2m (r4) и nocache (r4), чтобы получить по 3 чистых повтора на режим. Перед каждым прогоном записаны `uptime` и `ps aux | grep od-runner` (чужих `od-runner` не было), во время прогона каждые 30 с пишется load average за 1 мин; wall, user и max RSS — `/usr/bin/time -l`. **Правило загрязнения (задано до анализа):** прогон «загрязнён», если 1-минутный load average в любой точке выборки превысил 12 (14 ядер − 2) или код выхода не 0; загрязнённые прогоны сохранены и исключены из медиан.
- **Предзаданное правило решения (не менялось после измерений).** Менять умолчание на 2m, только если медиана wall у 2m меньше медианы у 4k на величину, превышающую разброс (max−min по чистым повторам у 4k и у 2m, ОБА), И относительный выигрыш ≥ 5 %; иначе оставить 4 KiB и записать «выигрыш не доказан».
- **Прогоны (11 штук).** Все 11: retired 2 321 128 949, стоп `user_entry`, sha256 отчёта (без sidecar/inputs, JSON с отсортированными ключами, как в S4-13d) `7687f27f…` и sha256 RAM `aa5da8bb…` — те же значения, что в S4-13d. Load «до/после» — 1-минутная величина `uptime`; max — максимум по выборкам раз в 30 с.

  | режим | повтор | старт | wall (s) | user (s) | max RSS (B) | load до / после | load max | статус |
  | --- | ---: | --- | ---: | ---: | ---: | --- | ---: | --- |
  | 4k | 1 | 03:52 | 1010,39 | 1002,06 | 638 156 800 | 3,84 / 3,29 | 6,36 | чистый |
  | 2m | 1 | 04:09 | 1023,02 | 1015,15 | 581 091 328 | 3,29 / 10,62 | 12,13 | загрязнён |
  | nocache | 1 | 04:26 | 1943,44 | 1930,01 | 577 191 936 | 10,62 / 3,50 | 10,62 | чистый |
  | 4k | 2 | 04:58 | 1017,27 | 1008,16 | 577 372 160 | 3,50 / 2,33 | 5,99 | чистый |
  | 2m | 2 | 05:15 | 1014,74 | 1008,31 | 581 648 384 | 2,33 / 2,97 | 5,01 | чистый |
  | nocache | 2 | 05:32 | 2015,18 | 1995,32 | 637 845 504 | 2,97 / 2,18 | 6,97 | чистый |
  | 4k | 3 | 06:06 | 1029,10 | 1015,94 | 581 107 712 | 2,18 / 3,21 | 7,57 | чистый |
  | 2m | 3 | 06:23 | 1033,33 | 1019,37 | 577 306 624 | 3,21 / 4,89 | 6,63 | чистый |
  | nocache | 3 | 06:40 | 1893,37 | 1848,23 | 577 339 392 | 4,89 / 4,02 | 24,11 | загрязнён |
  | 2m | 4 | 07:14 | 1036,79 | 1020,33 | 638 877 696 | 4,34 / 4,60 | 5,88 | чистый |
  | nocache | 4 | 07:32 | 1899,37 | 1861,45 | 582 156 288 | 4,60 / 3,92 | 9,26 | чистый |

  Загрязнены: 2m r1 (максимум 12,13 — на 0,13 выше порога, короткий всплеск load на границе прогонов 2m r1 / nocache r1 до 10,62; среднее по прогону 4,47) и nocache r3 (всплеск до 24,11, среднее 5,17). Источник всплесков не установлен (чужой процесс `od-runner` не наблюдался; список «топ по CPU» до и после прогонов сохранён только в сырых файлах scratchpad и не разбирался). Порог 12 выбран заранее и применён как есть; пограничный 2m r1 исключён по правилу, чувствительность к этому проверена ниже.

- **Медианы и разброс (только чистые повторы, по 3 на режим).**

  | режим | wall, медиана (s) | разброс wall max−min (s) | user, медиана (s) | разброс user (s) |
  | --- | ---: | ---: | ---: | ---: |
  | 4k | 1017,27 (1010,39 / 1017,27 / 1029,10) | 18,71 | 1008,16 | 13,88 |
  | 2m | 1033,33 (1014,74 / 1033,33 / 1036,79) | 22,05 | 1019,37 | 12,02 |
  | nocache (опорная) | 1943,44 (1899,37 / 1943,44 / 2015,18) | 115,81 | 1930,01 | 133,87 |

  Относительно 4k: **2m медленнее на 16,06 s, то есть на 1,58 %** (user: +11,2 s, +1,1 %); **nocache в 1,91 раза дольше** (+926,2 s, +91 %). Для 4k и 2m разброс повторов (18,7 и 22,1 s) больше модуля разницы медиан (16,1 s): различия во времени между 4k и 2m в шуме, а знак разницы — в пользу 4k. Чувствительность: если включить загрязнённые прогоны, медиана 2m (4 повтора) — 1028,18 s, разница с 4k −10,9 s (−1,1 %), nocache (4 повтора) — 1921,41 s (1,89 раза); вывод не меняется.
- **Применение правила решения.** (1) 2m НЕ быстрее 4k: разница медиан −16,06 s (отрицательная), условие «меньше медианы 4k на величину, превышающую оба разброса (18,71 и 22,05 s)» не выполнено; (2) относительный выигрыш −1,58 % < 5 %: условие не выполнено. **Вердикт: выигрыш не доказан, умолчание остаётся 4 KiB.** В этом замере тег 2 MiB (меньше обходов выборки в 4,4 раза, меньше промахов в 5,0 раза: фаза 2 — 7,6 M против 38,3 M) не дал ускорения: обходы и промахи выборки — не доминирующая по времени часть прогона. Причина по данным не установлена; гипотеза — основное время идёт на обходы данных (822 M против 12,0 M обходов выборки) и исполнение, а выигрыш выборки (9,3 M обходов fetch) в сравнении с ними мал; рост `write_flushes` (1 032 против 664) и другие эффекты тега не разбирались. Опорная: без кэша 1943,44 s, то есть сам кэш (4 KiB) ускоряет прогон в 1,91 раза.
- **Ограничения.** (1) Один хост (macOS arm64, 14 ядер), одна release-сборка (`1a63de3`), одна конфигурация (`stage4a-apic`, стоп `user_entry`, 2,33·10⁹); `--user-mode` и `stage4a` не мерились. (2) Фоновая нагрузка хоста не контролировалась: общий load во время прогонов 2–7 при 14 ядрах, со всплесками до 12–24 (два прогона из 11 загрязнены); чужой `od-runner` не было. Тактовая частота, температура и планировщик macOS не фиксировались. (3) Три чистых повтора на режим — малая выборка: разброс (max−min) от трёх точек грубая оценка; различия ≤ 2 % не разрешимы этим замером. Порядок прогонов — по кругу, но 2m r4 и nocache r4 идут после остальных (дрейф со временем возможен). (4) Sidecar включён во всех режимах: время включает накладные расходы sidecar (хэш RAM в конце, RSS ~ 550–640 MiB); без `OD_FETCH_CACHE_STATS` время не мерилось. (5) Время по фазам и по видам обходов не измерялось. (6) Порог загрязнения (12) выбран до анализа, но произволен; 2m r1 превысил его на 0,13. (7) Источник всплесков load не установлен. (8) Независимое ревью не проведено. Файл данных: `docs/reports/data/stage4/s4-13e-timing-2026-11-16.json`; сырые данные и скрипты — в сессионном scratchpad (не в репозитории).
- **Не закрыто:** чекбокс S4-13 не отмечен; умолчание кэша в коде НЕ менялось; применение и закрытие — S4-13f.

## S4-38L4i — opt-in запись checkpoints C0…C17 в цикле Linux-платы (только наблюдение; S4-38 и L4 открыты; ревью не проведено)

- **Файлы:** `crates/od-machine/src/linux_checkpoints.rs` (новый: `CheckpointRecorder`, `LinuxCheckpointTable/Target/Record`), `paged_memory_adapter.rs` (`run_cpl0_set_paged_2m_ram_observed` с телом прежнего цикла; `..._with_policy` стала обёрткой с `None`; общее чтение CPUID на границе; `observed_fetch_view`), `firmware_board.rs` (поле `checkpoints: Option<..>`, `enable_checkpoints`, `take_checkpoints`; `run_linux_boot_cpl0_set` не менялась, сигнатура та же), `linux_direct.rs` (`run_linux_direct_board_set_observed`, поле `LinuxStopRecord::checkpoints: Option<..>`), `lib.rs`, `tools/runner/src/linux_direct.rs` и `linux_direct/checkpoints.rs` (флаг `--checkpoint-table`, разбор оракула и cpio `/init`, ключ `checkpoints`). `od-isa`, CPUID-профиль, члены набора, гейт, cmdline, часы (`enable_clock_owner` не включался) и APIC не менялись.
- **Что записывается.** Регистратор получает вид границы инструкции (RIP, CPL, CR3, IDTR, EAX из `CPUID` по RIP) после проверок стопа, бюджета и IRQ, до проверки границы CPUID и шага; по умолчанию его нет, и цикл не строит вид. Запись: `id, name, kind, rip, gpa, retired, quanta, virtual_ns, cr3, cpl`, до 16 байт по `gpa`; пишется ТОЛЬКО при первом попадании. `quanta` — номер кванта границы (как `quanta` стоп-записи, если бы прогон встал тут); `virtual_ns` = время начала кванта + 1 нс на инструкцию (плата двигает часы после кванта). Предикаты: C0 — первая граница; C1 — адрес первой выборки relocated-копии (`relocated_copy` оракула); C2 — IDTR отличается от значения на первой границе (первый выполненный `LIDT`; граница после события); C3 — первый выполненный `MOV CR3` (граница после); C4 — первый доставленный гостю `#PF` (граница после, RIP — вход обработчика); C5 — `e_entry`; C6 — первый `CPUID` с EAX = 0; C7 — адрес `CPUID(1)` из оракула `0x139_c287` (без остановки на stage4a, на DiagnosticV1 это же граница стопа `cpuid_divergence`); C8–C15 — символы оракула; C16 — `kernel_execve`, только после C15; C17 — первая выборка в CPL3 по `e_entry` `/init` (читается из `initramfs.cpio`: `0x4000b0`; `/init` должен быть статическим ELF64 `ET_EXEC`, иначе C17 `not_reached` с причиной; засчитывается только после C16, чтобы usermode helper с тем же `e_entry` не считался). Адресный checkpoint засчитывается, если RIP равен VA или PA строки и трансляция RIP через текущий CR3 даёт PA строки (для C17 PA нет, проверяется только RIP и CPL 3).
- **Отступления от плана (записаны сознательно).** (1) C16 (`kernel_execve`) отдельно от C17 и засчитывается только после C15: usermode helper может вызвать `kernel_execve` раньше `run_init_process`; в плане он стоял под C17. (2) C17 — первая выборка в CPL3 по `e_entry` `/init`, а не отдельная запись исполненного `IRETQ`: без `--user-mode` `IRETQ` в CPL3 стопорит прогон как `user_entry`, и C17 тогда `not_reached` с этой причиной. (3) Запись на границе, где прогон кончается по бюджету или запросу стопа, не делается (проверки стопа и бюджета идут раньше); такую границу описывает стоп-запись. На границе стопа `cpuid_divergence` запись делается (регистратор вызывается до проверки границы CPUID, а не после, как предписано в задаче: иначе C7 на DiagnosticV1 не записался бы). (4) Сверка RAM по PT_LOAD для C5 не сделана целиком: сверяются только 16 байт по адресу строки оракула (`bytes_16`).
- **Проверка таблицы (по ревью Opus).** Публичная таблица `LinuxCheckpointTable` проверяется при создании регистратора (`validate`): `id` меньше 18, не id события (0, 2, 3, 4, 6: строка таблицы не может занять слот события), без повторов; `after` меньше 18 и не равен собственному `id`. Отказ — типизированная `LinuxCheckpointTableError`, на фасаде `run_failed` с `retired` 0 до первой инструкции; паники в цикле нет. Побочное действие регистратора: пик CPUID (общий с границей CPUID) при кэше выборки вызывает `FetchTranslationCache::lookup`, он двигает `recent` и статистику hits/misses; статистика видна только в `PagedCpl0SetReport::fetch_cache_stats` (читают тесты, не плата), на `LinuxSetRunReport` и прогон не влияет.
- **Ограничения.** C2 не видит `LIDT` с тем же значением IDTR. `retired` и `quanta` считаются с начала вызова `run_linux_boot_cpl0_set` (в зачётном прогоне вызов один). Для C17 `gpa` = `null`: первая выборка из `e_entry` приходит до подкачки страницы (ожидаемо `#PF`, из-за которого страница не отображена в момент границы; отдельно не проверялось).
- **Результаты на реальном ядре (release, бюджет 3·10⁹, evidence `docs/reports/data/stage4/s4-38l4i-checkpoints.json`).**
  - **stage4a, `--user-mode`, два холодных процесса:** отчёты побайтно равны (sha256 `0bddcc72…a4d4bc`), записи C0–C17 равны. Прогон БЕЗ флага: sha256 отчёта `d99b1898…53bd2a` — побайтно равен принятому `s4-39j-gate-counted-cold1-stage4a-run.json`; отчёт с флагом без ключа `checkpoints` равен отчёту без флага. Гейт на сохранённом отчёте — `PASS`, вердикт побайтно равен `s4-39j-gate-counted-verdict.json`.
  - **Достигнуты 17 из 18:** C0 retired 0; C1 `0x541b9c0` retired 1393; C2 `0x2066483` retired 119; C3 `0x543c247` retired 330182 (CR3 → `0x54a0000`); C4 `0x543c640` retired 390836; C5 `0x3f253e0` retired 1137474211; C6 `0x20669ff` retired 1363; C7 `0x139c287` retired 1137474358; C8 `x86_64_start_kernel` 1137477750; C9 `start_kernel` 1137485714; C10 `setup_arch` 1137498153; C12 `apic_intr_mode_init` 1546867493; C11 `tsc_init` 1546888467; C14 `populate_rootfs` 2071529329; C15 `run_init_process` 2319787639; C16 `kernel_execve` 2319814351; C17 `0x4000b0` в CPL3, retired 2320040851 (`cpl3_retired` прогона 796). Номера C-идентификаторов следуют таблице оракула, а не порядку вызова (C12 раньше C11). Статические прогнозы L2 подтверждены исполнением: relocated-копия (C1) и CPUID(1) (C7).
  - **`not_reached`: C13 `check_timer`** — на stage4a в CPUID не объявлен APIC и прерывания идут через 8259 (ISSUE-091), путь с `check_timer` не взят; причина именно в коде ядра не проверялась (в отчёте причина — «предикат не выполнился до конца прогона», `exit_kind` `budget_exhausted`). Прогон кончается по бюджету уже после C17 (`budget_exhausted`, `retired` 2999999998), как и принятый.
  - **Расхождение байт с оракулом (записать):** для C12, C14, C15 и C16 первые 5 байт в RAM — `0f 1f 44 00 00` (многобайтовый NOP), в оракуле по vmlinux — `e8 xx xx xx xx` (`call` на `__fentry__`). Это согласуется с заменой вызовов ftrace на NOP при загрузке ядра; отдельно не проверялось. Остальные 11 байт совпадают. Для C5, C7–C11 16 байт равны оракулу.
  - **DiagnosticV1 (без `--cpuid-profile`):** `cpuid_divergence`, RIP `0x139c287`, `retired` 1137474358, `quanta` 10131, расхождения с принятым значением 1137474358 (S4-38L6d) нет. C0–C7 записаны так же, как на stage4a (C0–C7 равны по `retired`), C8–C17 `not_reached` («предикат не выполнился до конца прогона»; C16 — «только после C15»).
- **Тесты (прошли):** `linux_checkpoints::tests` (10: первая запись C0 со смещением вызова, «только первый раз», PA/VA с проверкой трансляции, трансляция в другое место или её отсутствие — не попадание, адрес со сдвигом на 1 — `not_reached`, C16 не засчитан до C15 и засчитан после, C17 при чужом `e_entry` и при CPL 0 — `not_reached`, события C2/C3/C4 один раз и после события, C6 только для EAX = 0, трансляция запрашивается только при сработавшем предикате); `linux_direct::tests` (3: порядок записей и `retired` на плате по новому CR3 с `fetch_cache` вкл/выкл; `the_recorder_does_not_change_the_run`: `Debug` отчёта прогона равен без регистратора и с ним, в том числе при бюджете, обрывающем прогон после `MOV CR3`; фасад на CPUID-программе: C0, C6, C7, `retired`/`quanta` C7 равны стоп-записи, `LinuxStopRecord` равен прежнему при `checkpoints = None`); runner (7: нет ключа без флага; с флагом отчёт без ключа равен отчёту без флага; сдвиг адреса; чужой sha vmlinuz — отказ `checkpoint_table_vmlinuz_mismatch` код 4 до запуска; нечитаемая или неполная таблица — `invalid_input` код 3; флаг требует `--run --set-cpl0`; разбор cpio `/init`).
- **Мутация:** убрано «первое попадание» (запись каждый раз: адресная цель не удаляется из ожидающих и пропуск занятого слота снят) — падает `an_address_checkpoint_is_recorded_the_first_time_only` (retired 109 вместо 102), остальные 9 тестов модуля проходят. Откат копированием сохранённого файла, повторный прогон модуля зелёный.
- **Проверки:** `cargo fmt -p od-machine -- --check` ок; `cargo check --workspace --all-targets --offline` без ошибок (предупреждения старые: `page_walk.rs`, `od-host`); `cargo test -p od-machine --offline -- --test-threads=1`: 1253 passed, 6 ignored, 0 failed (до этого модульные тесты S4-38L4i отдельно); `cargo test -p od-runner --offline`: 0 failed.
- **Не закрыто:** независимое ревью Opus не проведено; чекбоксы S4-38 и L4 не отмечены; C13 на stage4a не достигается (ISSUE-091, этап 5); полная сверка RAM по PT_LOAD для C5; запись границы, на которой кончается бюджет; сопоставление байтов оракула и RAM после ftrace-патчей.

## S4-38L8d — профиль `X8664V1Stage4AApic`: APIC на Linux-плате, checkpoint `check_timer` (диагностический результат, не засчитывается; S4-38 открыт; ревью Opus: блокер и документация исправлены, повторное ревью не проведено)

- **Файлы:** `crates/od-isa/src/cpuid.rs` (вариант профиля, `LEAF1_EDX_APIC`, `is_stage4a_family`; только добавления), `crates/od-machine/src/bus.rs`, `page_walk.rs`, `paged_memory_adapter.rs` (список окон MMIO цикла набора), `firmware_board.rs` (`enable_pit_ioapic_irq`, `pit_ioapic_irq_available`), `linux_direct.rs` (включение профиля, тесты), `tools/runner/src/linux_direct.rs` (`--cpuid-profile stage4a-apic`, предикат семейства stage4a, `acceptance`/`diagnostic_note` нового профиля, ignored live-тест). Профиль `stage4a` не менялся.
- **Статус результата:** диагностический, не засчитывается (`acceptance` = `diagnostic_not_counted`); `PASS` S4-39j остаётся за `stage4a`. Единственный статус профиля — checkpoint L8d.
- **Команда прогона** (release, два холодных процесса, отчёты побайтно равны, sha256 `d0dc97d5d4efe49b70a651bff6940bb870673e95322691a9f76e9c12a484a4fa`): `od-runner linux-direct` с входами S4-39j (раздел S4-39j `docs/testing/MACHINE.md`), `--nonce s4-38l4a --ram-mib 128 --run --budget 2600000000 --set-cpl0 --cpuid-profile stage4a-apic --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json`. Evidence: `docs/reports/data/stage4/s4-38l8d-apic-check-timer.json`.
- **Результат.** Ключевые строки COM1 по порядку: `IOAPIC[0]: apic_id 1, version 32, address 0xfec00000, GSI 0-23`, затем `..TIMER: vector=0x30 apic1=0 pin1=2 apic2=-1 pin2=-1`; после них нет `MP-BIOS bug`, `trying to set up timer`, `IO-APIC + timer doesn't work`, `No local APIC present`. Доставлено 401 IRQ (`irqs_delivered`), фронтов PIT 228 (`irq0_edges`); часть доставок, вероятно, от таймера LAPIC, по вектору не разделялось. C13 `check_timer` reached (retired 1 547 625 421); C0–C13 reached, C14–C17 not_reached.
- **Стоп.** `exit_kind` `backend`: запись в IOWIN (`0xfec00010`) отвергнута IOAPIC (`UnsupportedConfiguration`), RIP `0xffffffff813f2f9d` (`mov [rax+0x10], r9d`), retired 1 792 955 123, CPL 0; r9 = `0xa020` (уровень, активный низкий, вектор 0x20, без маски) — вероятно SCI на pin 9 (по значению; IOREGSEL не проверялся). Это L8e. В журнале также `unchecked MSR access error: RDMSR from 0x1b` (ISSUE-502).
- **Замер `pending_vector` на границе** (release, `JMP $`, 20M инструкций, 7 повторов, без кэша выборки): без хука 393,4 нс/инстр. (min 392,3, max 402,1), хук с пустым источником 398,8 (+1,4 %), хук с источником PIT→IOAPIC→LAPIC 428,8 (+9,0 %, min 421,6, max 432,3; около +35 нс на границу). Оптимизация не делалась; решение по ней требует отдельного замера на реальном прогоне.
- **Тесты и мутация.** См. S4-38L8d в `docs/plans/STAGE_4.md`. Мутация (окна LAPIC/IOAPIC по умолчанию включены) роняет `stage4a_keeps_the_lapic_access_a_typed_error` и `the_set_loop_serves_ecam_and_no_other_mmio`; откат копией файла.
- **Не изменено:** отчёт stage4a `--user-mode` (бюджет 3·10⁹) побайтно равен принятому `s4-39j-gate-counted-cold1-stage4a-run.json` (sha256 `d99b1898da27fd283568b2c8bad4270233e75d103e20a19da2380842cf53bd2a`).
- **Не закрыто:** L8e (SCI pin 9), MSR 0x1B, `wake_ns` источника IOAPIC и пробуждение HLT по таймеру LAPIC (не проверено в живом прогоне), UART → INTIN4, user mode с этим профилем; повторное независимое ревью. См. ISSUE-091, ISSUE-502, ISSUE-503.

## S4-38L8e — SCI на IOAPIC pin 9 (level, active-low): запись RTE принята, прогон `stage4a-apic` дошёл до `kernel_execve` (диагностический результат, не засчитывается; реализовано, ожидает независимого ревью и принятия)

- **Файлы:** `crates/od-machine/src/io_apic.rs` (`valid_low`, чтение/запись RTE pin 9, `IoApicHandle::set_sci_line`, 6 тестов), `crates/od-machine/src/apic_pair.rs` (уровневая семантика на `LEVEL_PINS` = [4, 9], поля `sci_asserted`/`sci_remote_irr`), `crates/od-machine/src/linux_direct.rs` (2 гостевых теста), `docs/specs/MACHINE_CONTRACT.md`, `docs/testing/MACHINE.md`, `docs/KNOWN_ISSUES.md` (ISSUE-503), `docs/plans/STAGE_4.md`, evidence `docs/reports/data/stage4/s4-38l8e-sci-pin9.json`.
- **Решение:** принимается ровно форма, которую описывает ISO MADT (S4-31f: IRQ 9 → GSI 9, flags `0x000F`): pin 9, бит 15 (level) и бит 13 (active-low), без маски, Fixed, physical, вектор `0x10..=0xFE`. Всё остальное по-прежнему `UnsupportedConfiguration` без изменений снимка. Профиль stage4a окна IOAPIC не открывает; запись гостя там даёт прежнюю типизированную ошибку шины (`DeviceUnavailable` на `0xfec00000`).
- **Уровневая семантика.** Была только у INTIN4; обобщена на pin 9: доставка при снятой маске и поднятой линии ставит IRR/TMR и Remote IRR, EOI адресата снимает Remote IRR и, пока линия поднята и запись не маскирована, доставляет повторно; маска блокирует доставку, при Remote IRR второй запрос не ставится; вектор резервируется против фронтов и против уровневого INTIN4; Remote IRR накладывает вето на сброс IOAPIC и отключение LAPIC. Источника SCI в плате нет: линию вызывает только тест.
- **Живой прогон** (release, `linux-direct --run --budget 2600000000 --set-cpl0 --cpuid-profile stage4a-apic --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json`, один процесс, ~15 минут): запись `0xa020` принята; достигнуты C0-C16 (C16 `kernel_execve`, retired 2 321 216 080), не достигнут C17 `init_entry`. Журнал: `INT_SRC_OVR (bus 0 bus_irq 9 global_irq 9 low level)`, `ACPI: Using IOAPIC for interrupt routing`, `Run /init as init process`. Стоп: `exit_kind` `user_entry` (iretq в CPL3, RIP `0xffffffff81001268`, пользовательский вход `0x4000b0`, `Stopped`), retired 2 321 470 691, `virtual_ns` 3 590 462 691, `irqs_delivered` 1106, `memory_error` нет. Ядро по-прежнему видит ограничение MSR 0x1B (ISSUE-502).
- **Тесты.** Синтетические: `io_apic` (38, из них 5 новых и 1 переписанный), `linux_direct::apic_profile` `sci_rte` (2). Узкие наборы без ошибок: `io_apic`, `apic` (144), `diagnostic_vcpu` (46), `pit_ioapic` (16), `uart_irq` (4), `interleaved` (4). Мутация (убрано принятие polarity/trigger на pin 9 без маски): падают 6 тестов, включая принятие записи и доставку при снятии маски; откат копией файла. Полные прогоны `od-machine` и `od-runner` отложены до конца цели.
- **Не изменено:** отчёт stage4a `--user-mode` (бюджет 3·10⁹, release, на коде этого среза) побайтно равен принятому `s4-39j-gate-counted-cold1-stage4a-run.json` (sha256 `d99b1898da27fd283568b2c8bad4270233e75d103e20a19da2380842cf53bd2a`); `PASS` S4-39j остаётся за stage4a.
- **Не закрыто:** `wake_ns` источника IOAPIC и пробуждение HLT по таймеру LAPIC, UART → INTIN4, MSR 0x1B (ISSUE-502), user mode под APIC-профилем (ISSUE-090), доставка SCI гостю (источник SCI), независимое ревью. См. ISSUE-091, ISSUE-502, ISSUE-503.

## S4-38L8f — `wake_ns` источника PIT → IOAPIC (синтетические тесты; диагностический срез, не засчитывается; реализовано, ожидает независимого ревью и принятия)

- **Файлы:** `crates/od-machine/src/pit_ioapic_irq_source.rs` (`wake_ns`, 2 теста), `crates/od-machine/src/io_apic.rs` (`IoApicHandle::intin2_route`, только чтение), `crates/od-machine/src/local_apic.rs` (`LocalApicHandle::would_accept`, только чтение), `crates/od-machine/src/linux_direct.rs` (3 гостевых теста `apic_profile`), `docs/plans/STAGE_4.md`, `docs/KNOWN_ISSUES.md` (ISSUE-503).
- **Причина.** У `PitIoapicIrqSource` не было `wake_ns` (по умолчанию `None`), поэтому остановленный `HLT` с IF = 1 под `stage4a-apic` завершал прогон как `halted` (ISSUE-503, пункт 2).
- **Решение.** `wake_ns` по образцу `PitPicIrqSource`: после `catch_up` «сейчас», если у LAPIC есть доставляемый вектор; иначе срок следующего ребра OUT0, если оно было бы доставлено (entry 2 не замаскирован и направлен в этот LAPIC, LAPIC включён, класс вектора выше приоритета процессора по TPR и ISR); иначе `None`. Вопросы ничего не меняют сверх `catch_up`. Ветка простоя цикла платы не менялась (общая для обоих источников): IRQ идёт через LAPIC IRR и IDT, 8259 не участвует. LAPIC-таймер в ответ не входит (срок за владельцем часов, S4-23j).
- **Тесты.** `pit_ioapic` (18) и `apic_profile` (13 + 1 игнорируемый замер), `firmware_board` (92): без ошибок. Новые: пробуждение `STI; HLT` по PIT и доставка вектора entry 2 через IOAPIC и LAPIC с EOI гостя; замаскированный entry 2 не будит (прогон `halted`, `halt_wakes` = 0); вектор под TPR и без EOI, выключенный SVR — `None`; stage4a с `STI; HLT` по-прежнему `halted`; прежние тесты stage4a (`stage4a_keeps_the_*_typed_error`) без изменений. Мутация: `wake_ns` -> `None` роняет оба теста `wake_ns_is_*` и гостевой тест пробуждения; замена проверки маски роняет тест маски и гостевой тест маски; откат копией файла. Полные прогоны `od-machine` и `od-runner` отложены до конца цели.
- **Не изменено:** stage4a, его evidence и `PASS` S4-39j (sha256 `d99b1898da27fd283568b2c8bad4270233e75d103e20a19da2380842cf53bd2a`) не затронуты; `od-isa`, владелец часов и LAPIC-таймер не тронуты. Живой прогон `stage4a-apic` для этого среза не выполнялся.
- **Не закрыто:** LAPIC-таймер как источник пробуждения из HLT (в живом прогоне не проверен), MSR 0x1B (ISSUE-502), UART → INTIN4, L9, L10, user mode под APIC-профилем (ISSUE-090), независимое ревью. См. ISSUE-502, ISSUE-503.

## S4-38L8g — MSR 0x1B (IA32_APIC_BASE) под профилем `stage4a-apic` (диагностический срез, не засчитывается; реализовано, ожидает независимого ревью и принятия)

- **Файлы:** `crates/od-isa/src/execute.rs` (член набора `with_msr_apic_base`, ветка `0x1B` в обработке MSR), `crates/od-machine/src/linux_direct.rs` (`LinuxCpl0SetMembers::msr_apic_base`, включение только для `X8664V1Stage4AApic`, имя `msr:apic_base`, 3 гостевых теста `apic_profile`), `crates/od-isa/tests/cpl0_profile_set.rs` (4 теста), `docs/specs/CPU_CONTRACT.md`, `docs/plans/STAGE_4.md`, `docs/KNOWN_ISSUES.md` (ISSUE-502). `od-isa` тронут, потому что MSR выполняет цикл набора CPL0 (на уровне платы без `od-isa` `RDMSR` не перехватить); активных worktree S4-38 с правкой `od-isa` не было. Раннер не менялся (`..linux_default()`).
- **Причина.** ISSUE-502: ядро читает MSR 0x1B при перечислении CPU; `#GP(0)` давал `unchecked MSR access error`, `BSP APIC 0 is not marked in APICBASE MSR`, `Limiting to one CPU`.
- **Уточнение S6-21.** Согласованная правка CPL0 в `execute.rs` добавила выбор фиксированного значения MSR по роли vCPU. Для однопроцессорного профиля остаётся `0xFEE00900`. При включённом члене на AP используется `0xFEE00800`. [Результаты и проверка совместимости](STAGE_6_REPORT.md#s6-21--бит-bsp-в-ia32_apic_base-на-ap).
- **Решение.** `RDMSR` 0x1B: `EDX:EAX = 0:0xFEE00900` (база `0xFEE00000`, BSP бит 8, EN бит 11); `WRMSR` принимает только это значение (без состояния, меняется RIP), прочее — `Unsupported` до изменения состояния. Условия как у других MSR-членов (CPL0, Long64, голая форма без префиксов). Значение из SDM по памяти. Список членов stage4a, `minimal` и `linux_default` не менялся.
- **Тесты (узкие, полный `od-machine`/`od-runner` не запускались).** `od-isa` `cpl0_profile_set apic_base` (4), `od-machine --lib apic_profile` (16 + 1 игнорируемый), `od-machine --lib msr` (72): без ошибок. Проверяют точное `0xFEE00900`, приём совпадающей записи, отказ при иной базе, сброшенных EN/BSP, зарезервированных битах и верхней половине без изменения состояния, префиксы, отсутствие члена (stage4a и DiagnosticV1: `RDMSR` — `#GP(0)`, `WRMSR` — `Unsupported`). Мутация (`RDMSR` отдаёт 0 вместо базы): падают `rdmsr_0x1b_returns_*` и `rdmsr_apic_base_*`; откат копией файла.
- **Живой прогон** (release, `linux-direct --run --budget 2600000000 --set-cpl0 --cpuid-profile stage4a-apic --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json`, один процесс, ~15 минут; отчёт не сохранён в репозитории): в COM1 нет `unchecked MSR access error: RDMSR from 0x1b`, `BSP APIC 0 is not marked in APICBASE MSR`, `[Firmware Bug]: APIC enumeration order not specification compliant`, `Limiting to one CPU`. Стоп прежний: `user_entry` (iretq в CPL3, RIP `0xffffffff81001268`, вход `0x4000b0`), retired 2 321 128 410, достигнуты C0-C16, C17 не достигнут, `memory_error` нет, `set_members` содержит `msr:apic_base`. Один CPU остаётся по MADT (`Allowing 1 present CPUs`).
- **Не изменено:** stage4a (для него 0x1B по-прежнему `#GP(0)`), его evidence и `PASS` S4-39j (sha256 `d99b1898da27fd283568b2c8bad4270233e75d103e20a19da2380842cf53bd2a`). Повторного прогона stage4a `--user-mode` в этом срезе не было: проверена только неизменность членов и поведения тестами.
- **Не закрыто:** пробуждение HLT по таймеру LAPIC, UART → INTIN4, user mode под APIC-профилем (ISSUE-090), N vCPU (S6-18/S6-21), независимое ревью. ISSUE-502 закрыта кодом, тестами и прогоном.

## S4-38L8h — UART → IOAPIC INTIN4 под профилем `stage4a-apic` (синтетические тесты; диагностический срез, не засчитывается; реализовано, ожидает независимого ревью и принятия)

- **Файлы:** `crates/od-machine/src/uart_ioapic_irq_source.rs` (новый), `crates/od-machine/src/firmware_board.rs` (`enable_uart_ioapic_irq4`, поле `uart_ioapic_source`, выборки в цикле набора), `crates/od-machine/src/lib.rs`, `crates/od-machine/src/linux_direct.rs` (установка для `X8664V1Stage4AApic`, тесты `apic_profile`), `docs/plans/STAGE_4.md`, `docs/KNOWN_ISSUES.md` (ISSUE-503). `od-isa`, `io_apic.rs`, `apic_pair.rs`, `uart_pic_irq_source.rs` не тронуты.
- **Причина.** ISSUE-503, пункт 3: UART не был подключён к INTIN4; под `stage4a-apic` `enable_uart_irq4` (8259) возвращает `false`.
- **Решение.** `UartIoapicIrqSource` оборачивает `PitIoapicIrqSource` и передаёт уровень выхода прерывания UART как уровень INTIN4 существующим `IoApicHandle::set_intin4`; новой семантики нет (уровневая форма: IRR и TMR, Remote IRR до EOI, повтор при поднятой линии; фронтовая: один фронт на подъём; маска: нет запроса). Выборка на каждом вопросе цикла, до и после накачки THR и до `wake_ns`; отказанный подъём повторяется. Доставка, `acknowledge`, срок и пробуждение — обёрнутого источника PIT: вектор идёт через LAPIC IRR и IDT гостя. Источник ставится после источника таймера и только для профиля `X8664V1Stage4AApic`; с 8259-вариантом несовместим в обе стороны.
- **Тесты (узкие, полный `od-machine`/`od-runner` не запускались).** `od-machine --lib uart` (59), `--lib intin4` (9), `--lib apic` (162, 1 игнорируемый), `--lib io_apic` (40), `--lib irq_` (86): без ошибок. Гость (SVR, RTE 4, MCR OUT2, IER RDA, STI) получает событие RX через INTIN4 → LAPIC IRR → IDT, обработчик читает RBR и пишет EOI: одна доставка, ISR очищен, 8259 нетронут; маска RTE 4 → 0 доставок; уровневая форма с активной линией повторно доставляется после EOI, со сброшенной линией — один раз; фронтовая форма — один раз; остановленный `HLT` просыпается по ожидающему байту RX (`halt_wakes` = 1), при тихом UART не просыпается. Прежние `uart_irq4` (stage4a, 8259 IR4) без изменений. Дополнительно (ревью): повтор отказанного подъёма (уровневая RTE 4 при SVR=0x0FF, затем SVR=0x1FF: одна доставка, `uart_refused` = 1, `uart_edges` = 1) и THRE через INTIN4 (N байт: N+1 доставок, N байт в COM1; `--lib intin4` теперь 11). Мутации: `sample()` без действия роняет 6 из 9 базовых тестов `intin4` и 3 теста `uart`; `level.set(line)` при `Err` роняет тест повтора; снятие выборки перед накачкой THR роняет тест THRE; откат копиями файлов.
- **Не изменено:** stage4a (8259 IR4, S4-39i), его evidence и `PASS` S4-39j; `od-isa`.
- **Не закрыто:** живой прогон UART под `stage4a-apic`; пробуждение HLT по таймеру LAPIC в живом прогоне; вход в цикл платы при `Halted`: Прогон, который начинается при остановленном (`Halted`) ЦП, а источник уже имеет доставляемый вектор (например, байт RX подан между двумя вызовами цикла), завершается `run_failed` («an external interrupt of the board source was refused by the CPU delivery», retired 0); байт остаётся в RBR. Это свойство цикла платы для любого источника (PIT, 8259, UART), а не только UART; исправление — вход в цикл платы при `Halted` (общий блок с веткой после кванта) — открыто, до будущего среза с живым вводом RX или прогоном кусками. Не блокер для живого прогона: фасад вызывает плату один раз, RX во время прогона не подаётся. Ограничение: выборка UART происходит только при `members.irq` (источник опрашивается только с хуком прерываний); user mode под APIC-профилем (ISSUE-090); независимое ревью.

## S4-38L8i — пробуждение HLT по таймеру LAPIC под профилем `stage4a-apic` (синтетические тесты; диагностический срез, не засчитывается; реализовано, ожидает независимого ревью и принятия)

- **Файлы:** `crates/od-machine/src/firmware_board.rs` (`lapic_timer_wake_ns`, `idle_wake_ns`, ветка `Woken` в `idle_to`), `crates/od-machine/src/linux_direct.rs` (тесты `apic_profile`), `docs/plans/STAGE_4.md`, `docs/KNOWN_ISSUES.md` (ISSUE-503). `od-isa`, `clock_owner.rs`, `idle_arbiter.rs`, `local_apic.rs` не тронуты.
- **Причина.** ISSUE-503, пункт 2: владелец часов и арбитр учитывали срок таймера LAPIC, но ветка простоя цикла платы запускалась только при `irq_source.wake_ns() = Some`, а источник IOAPIC таймер LAPIC не учитывает: `HLT` с IF = 1 при армированном таймере LAPIC и замаскированном PIT завершал прогон как `halted`. `idle_to` к тому же отвергал вектор LAPIC, будящий ЦП (`Woken`).
- **Решение.** `idle_wake_ns` = минимум срока источника платы и `lapic_timer_wake_ns` (срок таймера LAPIC BSP при незамаскированном LVT и принимаемом векторе; только с владельцем часов и источником IOAPIC). `idle_to` под источником IOAPIC принимает `Woken`: ЦП снова `Running`, следующая граница доставляет вектор из LAPIC IRR через IDT гостя. Под 8259 и без владельца часов поведение прежнее.
- **Тесты (узкие).** `od-machine --lib apic_profile` (32, 1 игнорируемый), `--lib clock_owner` (22), `--lib pic_wake` (15), `--lib idle_arbiter` (15), `opt_in_refuses` (2): без ошибок; `cargo fmt --check`, `cargo check --all-targets` чисты. Синтетический гость: one-shot (1000 нс): одно пробуждение, часы стоят ровно на сроке, вектор через IRR/IDT, EOI, код после `HLT` выполнен; periodic: пробуждение на каждый период; замаскированный LVT: `halted`; stage4a: запись LAPIC MMIO остаётся типизированной ошибкой; TPR того же класса, что вектор таймера: `halted`; PIT открыт, срок LAPIC раньше: пробуждение по LAPIC. Мутации: `lapic_timer_wake_ns` всегда `None` роняет оба теста пробуждения; снятие `would_accept` роняет тест TPR; замена `min` на `source.or`/`max` в `idle_wake_ns` тесты не роняет (эквивалентна: арбитр сам берёт более ранний срок таймера LAPIC); откат копиями файла.
- **Не изменено:** stage4a, 8259 путь, их evidence и `PASS` S4-39j; `od-isa`.
- **Не закрыто:** живой прогон `stage4a-apic` с пробуждением по таймеру LAPIC; независимое ревью; вход в цикл платы при `Halted` (см. L8h); user mode под APIC-профилем (ISSUE-090).

## S4-38L9-um — user mode под профилем `stage4a-apic` (синтетические тесты; диагностический срез, не засчитывается; реализовано, ожидает независимого ревью и принятия)

- **Файлы:** `crates/od-machine/src/linux_direct.rs` (снят отказ, тесты `apic_profile`, фикстура `user_mode_board_with`), `tools/runner/src/linux_direct.rs` (разбор `--user-mode`, тесты), `docs/plans/STAGE_4.md`, `docs/KNOWN_ISSUES.md` (ISSUE-503, ISSUE-505). `od-isa`, `paged_memory_adapter.rs`, `firmware_board.rs` не тронуты.
- **Причина.** ISSUE-503, пункт 4: `--user-mode` с `stage4a-apic` отвергался (`run_failed` до изменения платы; в раннере отказ при разборе), потому что CPL3-вход IRQ был построен под 8259. ISSUE-090 для 8259 закрыта (`od-isa`, `994d522`): внешний IRQ из CPL3 доставляется через `TSS.RSP0`.
- **Решение.** Вход IRQ на границе набора (`deliver_pending_irq`) не зависит ни от источника вектора, ни от привилегии: `pending_vector` источника IOAPIC отдаёт старший доставляемый вектор LAPIC (от PIT -> INTIN2 и от таймера LAPIC), `deliver_external_interrupt_cpl0_set` строит CPL3-кадр на RSP0. Потому достаточно снять отказ: в `run_linux_direct_board_set_with_post_iret_index` и в разборе раннера (`stage4a` и `stage4a-apic`; диагностический профиль отвергается, сообщение: «--user-mode needs --run, --set-cpl0 and --cpuid-profile stage4a or stage4a-apic»). Под APIC результат остаётся `diagnostic_not_counted`; stage4a и его counted `PASS` S4-39j (sha d99b1898...) не затронуты.
- **Тесты (узкие).** Синтетический CPL3-код `MOV EAX, 0; JMP -7`; шлюз IRQ на обработчик CPL0 (EOI записью в LAPIC, `IRETQ`), `TSS.RSP0`, окна APIC в таблицах страниц. (a) таймер LAPIC, armed хостом (200 нс): одна доставка, CPL 3 при выходе, `cpl3_retired` > 900, RSP восстановлен, кадр на RSP0 (SS 0x1b, RSP пользователя, RFLAGS.IF, CS 0x13, RIP пользователя), ISR очищен; (a2) PIT -> INTIN2 (настройка APIC гостем в CPL0 до `IRETQ`): >= 2 доставок, `acknowledged` источника равен `irqs_delivered`, отказов нет; (b) IF = 0 в CPL3: `irqs_delivered` = 0, `irq_refused_masked` > 100, без `run_failed`, вектор остаётся в IRR; (c) stage4a `--user-mode`: тесты S4-39c-f не менялись и проходят. Раннер: разбор (`stage4a-apic --user-mode` принимается, `diagnostic` и без профиля отвергаются), сквозной тест `user_mode_under_the_apic_profile_runs_into_cpl3_and_is_not_counted`. Мутация: пропуск входа IRQ в CPL3 роняет (a) и (a2); откат копией файла.
- **Наблюдение (не исправлено, вне среза).** Запись гостя в регистры таймера LAPIC не заканчивает квант цикла платы: таймер, армированный гостем внутри кванта, диспетчеризуется на следующей границе кванта (граница квантов задаётся `HLT`, чтением RDTSC/PM-таймера, записью в PIT, дедлайном). Для живого прогона user mode это значит, что тик планировщика на спин-цикле CPL3 без таких событий доставляется с задержкой до конца кванта. Поэтому в тестах (a) таймер армирует хост. Нужен отдельный срез, если живой прогон это покажет.
- **Не закрыто:** живой прогон `stage4a-apic --user-mode` (не выполнялся); независимое ревью; L9, L10.

## S4-38L9J — формальное evidence discovery на профиле `stage4a-apic` (диагностика, не засчитывается; реализовано, ожидает независимого ревью и принятия)

- **Файлы:** `tools/runner/src/linux_discovery.rs` (новый), `tools/runner/src/main.rs` (команда `linux-direct-discovery`), `docs/reports/data/stage4/s4-38l9j-apic-discovery-run.json`, `s4-38l9j-apic-discovery-verdict.json`, `docs/plans/STAGE_4.md`, `docs/KNOWN_ISSUES.md` (ISSUE-091, ISSUE-503). Код `od-machine`, `od-isa`, CPUID и поведение PIT/strobe не менялись.
- **Что сделано.** Проверка отчёта `linux-direct-discovery --report <json> [--out <json>]` по образцу `linux-direct-gate` (гость не запускается). Вердикт только диагностический: `PASS_DIAGNOSTIC_NOT_COUNTED` (код 6) или `FAIL` (5), `counted` всегда `false`. 40 проверок: форма прогона (3), `e820_journal` (журнал равен массиву `e820` отчёта), 15 обязательных маркеров (`Allowing 1 present CPUs plus 0 hotplug CPUs` — точная строка), 20 запрещённых (19 подстрок, в том числе `Disabling APIC timer`, `APIC frequency too slow, disabling apic timer`, `Spurious LAPIC timer interrupt`, `spurious APIC interrupt`, `unexpected IRQ trap`, `No irq handler for vector`, `WARNING:`, `BUG:`, `Oops`, и `Kernel panic`, допустимая только как `Attempted to kill init! exitcode=0x00000000`), порядок checkpoints C13 → C14 → C16 → C17. Журнал syscall `/init` вердиктом не проверяется. `Refined TSC` идёт в `observations`.
- **Тесты.** `cargo test -p od-runner --offline --bin od-runner linux_discovery`: 12 passed (по одному FAIL на каждый обязательный и каждый запрещённый маркер, двойники маркеров, e820, checkpoints, форма прогона, аргументы, коды выхода и `--out` у `run()`). Негатив на реальных файлах: `s4-38l8e-sci-pin9.json` — `FAIL` (RDMSR 0x1b, APICBASE, Firmware Bug, Limiting to one CPU; нет `user_mode`; C17 не достигнут); `s4-39j-gate-counted-cold1-stage4a-run.json` — `FAIL` (профиль stage4a; нет IOAPIC, INT_SRC_OVR, Using IOAPIC, TIMER pin 2; нет checkpoints).
- **Живой прогон** (release, два независимых холодных процесса, около 16 минут, параллельно): `--run --set-cpl0 --cpuid-profile stage4a-apic --user-mode --checkpoint-table docs/reports/data/stage4/s4-38l2-oracle.json --budget 3000000000 --nonce s4-38l9j --ram-mib 128`, входы как в `docs/testing/MACHINE.md` (S4-39j). Отчёты побайтно равны, sha256 `78a62bdbb2859053986425c615905781e71277c77fdefea7922da7e996ebdc24`. Вердикт: `PASS_DIAGNOSTIC_NOT_COUNTED`, 40 из 40, `failures` пусты (вердикт перегенерирован после ревью с расширенным списком запрещённых маркеров; ни один новый не сработал на журнале). Наблюдения (вердиктом не проверяются, кроме C13 → C14 → C16 → C17): C13 `check_timer` (retired 1 547 026 238), C14 `populate_rootfs` (2 072 822 955), C15 (2 320 611 408), C16 `kernel_execve` (2 320 644 462), C17 `init_entry` (2 320 901 822); журнал syscall 83, 165, 257, 0, 3, 1, 39, 35, 60; `OD_NONCE=s4-38l9j`; `..TIMER: vector=0x30 apic1=0 pin1=2 apic2=-1 pin2=-1`; `tsc: Detected 999.951 MHz processor`; `Refined TSC` в журнале не было; irqs_delivered 1113, halt_wakes 2, `cpl3_retired` 796.
- **Остановка прогона.** `exit_kind: backend` (retired 2 322 382 342, бюджет не исчерпан): уже ПОСЛЕ паники `Attempted to kill init` запись `0x10000` (LVT Timer: маска, вектор 0; `rdi` = `0x320`, `rsi` = `0x10000`) по `0xfee00320` из CPL0 (RIP `0xffffffff813fb122`) отвергнута моделью LAPIC как `UnsupportedConfiguration`. Это отличие от counted stage4a (там `budget_exhausted` в цикле паники). На проверки не влияет (все маркеры и checkpoints раньше), но стабильность на APIC не доказана; записано в ISSUE-503, пункт 5, как открытое.
- **Регрессия stage4a.** Counted-прогон не повторялся (срез не меняет код машины); принятый `s4-39j-gate-counted-cold1-stage4a-run.json` и `…cold2…` сверены по sha256: `d99b1898da27fd283568b2c8bad4270233e75d103e20a19da2380842cf53bd2a`, совпадает с `docs/testing/MACHINE.md`.
- **Не закрыто:** результат не засчитывается (ISSUE-091: профиль по умолчанию остаётся `stage4a`, ISSUE-068 открыта); запись LVT Timer (маска, вектор 0) после паники; независимое ревью. Полные `od-machine` и `od-runner` не запускались (узкие проверки: `cargo fmt -p od-runner -- --check`, `cargo check --workspace --all-targets --offline`, 12 тестов модуля).

## S4-15b — memory-блок JIT привязан к коду в guest RAM (реализовано; ревью Claude Opus, high — принято с ограничениями; итог S4-15 — в конце раздела)

- **Зачем.** После S4-15a JIT на `MachineBus` исполнял ровно `48 8B 03`, но блок компилировался из константы байтов: код не читался из guest RAM, права выборки (execute/NX/U/S) и A-бит кода не участвовали.
- **Что сделано.** В `crates/od-host/src/lib.rs` добавлена сессия `PagedFetchedLoadSession` (+ `PagedFetchedLoadReport`, `PagedFetchedLoadError{,Kind}`, `PagedFetchAdmission`, `PAGED_FETCHED_LOAD_V2_BYTES`). Каждый `execute` сначала проверяет контекст/CR3/операнд (без page walk), затем делает tracked instruction fetch 3 байт по RIP (`MachineBus::tracked_code_snapshot_with_callback`, тот же walker, что у fetch интерпретатора: execute/NX/U/S для CPL, A-биты кода коммитятся как effect и возвращаются в `fetch_committed_writes`). Snapshot хранит `physical_base` и `backing_generation` (`PhysicalMemory::code_page_generation`). Блок компилируется (`compile_memory_load_block_v2`) и публикуется из байтов snapshot с `entry_rip` = RIP. Memory-операнд исполняется прежним `execute_paged_mov_rax_rbx_v2` → `with_paged_mov_memory` (scoped `GuestMemory`, без direct host pointer). Проверки CPU-контекста вынесены в приватный `check_paged_load_v2_context` без изменения порядка/поведения старого пути; остальные существующие пути не менялись.
- **Допуск формы.** Ровно `48 8B 03` (3 байта в одной guest-странице). Иначе `UnsupportedForm`/`Fetch(...)`, константа не исполняется. Ранее допущенный блок сбрасывается при `UnsupportedForm` (а также при отказе компиляции/публикации) и при изменении generation/байтов/страницы; при `Fetch(...)` (NX, not-present, U/S, InvalidLength, NotRam) выход происходит до сброса, блок остаётся в сессии и держит MAP_JIT регион, но исполнения без нового совпадающего fetch не происходит.
- **Повторный допуск.** Блок переиспользуется, только если совпали линейный RIP, физическая страница, generation страницы и байты; иначе старый блок освобождается (в хосте один MAP_JIT регион) и форма допускается заново из новых байт. Fetch с проверкой прав выполняется на каждом вызове, поэтому NX/снятие прав на живой странице даёт #PF без исполнения. Запись даже того же значения увеличивает generation и вызывает повторный допуск (консервативно).
- **Ограничение: только 4 KiB-страницы кода.** Повторный допуск и fetch работают только для 4 KiB-страниц кода: `tracked_code_snapshot_with_callback` идёт через 4 KiB walker (`bus.rs` ~896–903, `access_long64_4k_ram`); PDE с PS=1 даёт типизированный отказ `Fetch(Access(UnsupportedPageSize))`, возможно с уже закоммиченными A верхних уровней. Код ядра Linux обычно отображён 2 MiB-страницами — на реальном ядре путь почти наверняка будет отказывать. Поддержка 2 MiB-страниц кода — отдельный срез (S4-15e или S4-16).
- **Тесты** (`tests/system/stage4_memory_fault.rs`, рядом с `s4_17a`; macOS arm64; интерпретатор `step_paged_mov` на независимой идентичной RAM — oracle для parity: 16 GPR, RIP, masked RFLAGS, `PageFaultRecord`, упорядоченные committed writes, весь образ RAM):
  1. `s4_15b_block_is_admitted_from_guest_ram_bytes_and_matches_the_interpreter` — parity, Admitted→Reused, второй RIP/страница/смещение (блок от константного RIP дал бы `EntryRipMismatch`).
  2. `s4_15b_unsupported_bytes_in_ram_are_refused_not_replaced_by_a_constant` — `48 8B 0B` и NOP в RAM: отказ, RAX/RIP/data-PTE не тронуты, старый блок сброшен.
  3. `s4_15b_fetch_faults_precede_any_data_access_and_match_the_interpreter` — NX@CPL0 (PFEC 0x11), supervisor-only@CPL3 (0x15), not-present@CPL0 (0x10): CR2=RIP, saved RIP, RAX неизменён, data-ветка (A-биты) нетронута, writes/RAM равны интерпретатору.
  4. `s4_15b_code_accessed_bits_are_committed_without_preset_and_data_fault_matches` — код-ветка без предустановленных A: 4 записи PML4/PDPT/PD/PT, A=1, D=0; CPL0/CPL3 и #PF по data-leaf (PFEC 0, RAX не тронут, A кода остаётся); повторный вызов — без новых fetch-записей, Reused.
  5. `s4_15b_patched_code_byte_forces_readmission_and_never_runs_the_old_block` — tracked patch другого байта: отказ, старый блок не исполнен; возврат байта и запись того же значения: generation растёт, Admitted; remap PTE на другую страницу с теми же байтами: Admitted; NX на живой странице: #PF 0x11.
  6. `s4_15b_reuse_key_compares_bytes_when_page_and_generation_are_equal` — один и тот же RIP, физическая страница и generation на двух разных `MachineBus` (по одной tracked-записи в код-страницу на каждом), байты `48 8B 03` против `48 8B 0B`: отказ `UnsupportedForm`, старый блок не исполнен и сброшен.
  - Мутация сравнения байтов (убрать только `&& admitted.bytes == fetched_bytes`): падает тест 6 (`Reused` на байтах `48 8B 0B`), остальные проходят; откачено, файл совпал с копией (`cmp`).
  - Мутации (проверены вручную, затем откачены): убрать сравнение generation → падает тест 5; компилировать от константного RIP → падает тест 1; fetch всегда с Ring0 → падает тест 3; не сбрасывать старый блок при отказе формы → падают тесты 1, 2, 5.
- **Результаты.** `cargo test -p od-host --offline --test stage4_memory_fault` — 7 passed (6 новых + s4_17a); `--lib s4_15` — 1 passed (s4_15a); `cargo test -p od-jit --offline --lib` — 455 passed, 2 ignored; `cargo check --workspace --all-targets --offline` — exit 0.
- **НЕ сделано (S4-15c–f/g):** поддержка 2 MiB-страниц кода (повторный допуск/fetch только для 4 KiB; S4-15e или S4-16), store, расширение допустимых форм и многоинструкционные блоки, MMIO-операнд и cross-page код/операнд, reset/retirement и live handle, fallback в интерпретатор для unsupported форм на уровне диспетчера, кэш нескольких блоков, повторный допуск при инвалидации через aliases (S4-16), перформанс. S4-15 и Stage 4 остаются открытыми.
- **Риски.** Одна сессия держит один опубликованный блок (ограничение MAP_JIT); перекрытие page-table записи с физической страницей кода меняет generation и вызывает лишний повторный допуск (безопасно, но не бесплатно); цена tracked fetch на каждый вызов не измерялась, speed claim не делается.
- **Ревью (Claude Opus, high): принято с ограничениями.** P3 закрыты: текст про ошибку fetch, тест на ключ повторного допуска по байтам (тест 6), ограничение «только 4 KiB-страницы кода» записано, `unwrap` заменён на `unwrap_or_else`. Остаются записанные ограничения: 2 MiB-страницы кода, один MAP_JIT-регион, цена tracked fetch не измерена.

## S4-15c — fallback-диспетчер JIT → interpreter на один шаг (реализовано; ревью Claude Opus, high — принято с ограничениями; итог S4-15 — в конце раздела)

- **Зачем.** После S4-15b все отказы JIT (`UnsupportedForm`, `UnsupportedOperandRange`, MMIO-операнд, fetch-отказы) возвращались вызывающему как ошибка; «Готово» S4-15 требует перехода unsupported форм в рабочий interpreter.
- **Что сделано.** В `crates/od-host/src/lib.rs` рядом с `PagedFetchedLoadSession` (существующие пути не менялись) добавлены `PagedFetchedLoadSession::execute_or_interpret(state, bus, cr3, efer, access_context, rflags_defined)`, `PagedStepOutcome::{Jit(PagedFetchedLoadReport), Interpreter{reason, report, committed_writes}}` (+ `path()`), `PagedStepPath`, `PagedStepFallbackReason::{Refused(PagedFetchedLoadErrorKind), OperandMemoryError(..)}`, `PagedStepError::{Interpreter(PagedMovStepError), JitHostFailure(..)}`.
- **Правило выбора пути.**
  - JIT исполнил инструкцию (`Complete` или `GuestFault` data #PF с уже закоммиченными A) → результат JIT, интерпретатор не вызывается.
  - JIT отказал, не исполнив инструкцию → ровно один `od_machine::step_paged_mov` на том же state/bus. Сюда входят: `Load(..)` (контекст, CR3, cross-page операнд, paging; до page walk), `Fetch(..)` (включая fetch #PF; интерпретатор повторяет walk и выдаёт тот же `PageFaultRecord`), `UnsupportedForm`, `Compile`, `Publish` (в т.ч. `MappingAlreadyLive`), и `OperandMemoryError` (RAM-only адаптер отказал по операнду, например MMIO: JIT-helper оставляет CPU state неизменным, `unchanged()` проверяется при декодировании результата, устройство не вызывается).
  - Единственные возможные effects таких отказов — A-биты page walk (fetch и/или data walk). Повторный walk их не коммитит снова (проверено: интерпретатор после отказа JIT получает 0 commit-колбэков). Writes отказавшей попытки добавляются в начало `committed_writes`, поэтому список равен списку чистого интерпретатора на идентичной RAM.
  - Любой другой отказ helper-границы JIT (`InvalidAccounting`, `HelperBoundaryFailure`, `InvalidSite`, …) — `JitHostFailure`, состояние не доказано неизменным, интерпретатор НЕ вызывается. Успешный результат JIT принимается явным сопоставлением только для `Complete` и `GuestFault`; прочие выходы (`CodeInvalidated`, `BudgetExhausted`, `StopRequested`, `StopAddress`) для блока из одной загрузки невозможны и дают `JitHostFailure(InvalidOutcome)`. Ветки `JitHostFailure` тестом не покрыты: из публичного API их не вызвать без правки кода/внедрения неисправного helper (ограничение, не закрыто).
  - `rflags_defined` проверяется до JIT-попытки: некорректный аргумент не может проявиться после fetch-effects.
- **Отклонение от формулировки задания.** Fetch #PF с уже закоммиченными A верхних уровней трактуется как отказ БЕЗ effects инструкции (A-биты идемпотентны), поэтому уходит в интерпретатор, который выдаёт тот же #PF в единообразном виде; не как «отказ после эффектов». Data #PF после исполненного JIT-шага интерпретатор не повторяет. Если ревьюер хочет, чтобы fetch #PF возвращался как ошибка JIT без интерпретатора, это однострочное изменение.
- **Что делает интерпретатор с формами.** Allowlist `step_paged_mov` — только `48 8B 03` и `48 89 0B`. NOP и `48 8B 0B` дают его явный `RunExit::Unsupported`, 0 retired, RIP/RAX не тронуты (диспетчер возвращает это в `report`, не скрывая); `48 89 0B` (store) исполняется интерпретатором один раз.
- **Тесты** (`tests/system/stage4_memory_fault.rs`; oracle — `step_paged_mov` на независимой идентичной RAM; сравниваются 16 GPR, RIP, masked RFLAGS и весь `LongModeState` целиком, `PageFaultRecord`/весь `PagedMovStepReport` для пути Interpreter, упорядоченные committed writes и весь образ RAM):
  1. `s4_15c_supported_form_stays_on_the_jit_and_equals_the_interpreter` — CPL0, CPL3, data #PF: путь Jit, равенство oracle. Фикстура кладёт в RAM по `CODE_PHYSICAL + 3` вторую инструкцию `48 89 0B` (store RCX в data qword), которую интерпретатор исполняет; поэтому лишний интерпретаторский шаг после JIT виден в RIP, data qword и D-бите. Мутация «лишний `step_paged_mov` после успешного JIT-шага перед `return Ok(Jit(..))`» → тест 1 падает (кейс «success CPL0», `dispatcher == pure interpreter`); до этой правки фикстуры (нули по RIP+3) мутация не ловилась.
  2. `s4_15c_fetch_fault_is_the_interpreters_page_fault_with_the_fetch_writes_kept` — NX fetch: путь Interpreter, `PageFaultRecord` из JIT-отказа `Fetch(Access(PageFault{record}))` равен записи интерпретатора (vector 14, PFEC 0x11, CR2=saved_rip=RIP, access Fetch, Ring0/Explicit), три A-записи верхних уровней по одному разу, data-ветка нетронута.
  3. `s4_15c_mmio_operand_reaches_the_device_exactly_once_via_the_interpreter` — JIT отдельно: `MemoryError`, state неизменён, 0 вызовов устройства, 4 data-A закоммичены; затем interpreter-шаг: 0 повторных commit-ов, образ RAM не изменился, 1 чтение устройства; диспетчер на свежей шине: путь Interpreter, `OperandMemoryError`, ровно 1 `Read(offset 0x120, qword)`, RAX = значение устройства, RIP+3, writes = 4 code + 4 data без дублей, A=1, D=0. Мутации: «MemoryError как итог JIT» → падает; «интерпретатор повторно с исходного RIP» → падает счётчик устройства.
  4. `s4_15c_cross_page_operand_is_refused_by_the_jit_before_the_walk_and_interpreted` — qword на границе страниц: `Refused(Load(UnsupportedOperandRange))`, ничего не допущено в сессию, интерпретатор читает обе страницы (RAX = `0x00038b4811223344`).
  5. `s4_15c_unsupported_forms_are_interpreted_once_or_reported_unsupported` — NOP и `48 8B 0B`: путь Interpreter, `Unsupported`, нет эффектов; `48 89 0B` в RAM: store исполнен, D на листе; тот же store в MMIO: ровно одна запись устройства. Мутация «нет интерпретатора для UnsupportedForm» → падает.
  6. `s4_15c_invalid_rflags_definedness_is_refused_before_any_effect` — ошибка до fetch: state и RAM (в т.ч. A-биты) не изменены.
  7. `s4_15c_publish_refusal_falls_back_to_the_interpreter` — живой блок другой сессии: точное `Refused(Publish(MappingAlreadyLive))` после fetch walk → интерпретатор, writes code(4)+data(4) по разу.
  - Мутации (вручную, затем откат из копии, `cmp` после отката): без интерпретатора на `UnsupportedForm` → падает тест 5; MMIO-MemoryError как итог → тест 3; лишний вызов `step_paged_mov` после исполненного JIT-шага → падает тест 1 (после правки фикстуры, см. выше); без добавления writes попытки → тесты 2, 3, 5, 7; повторный вызов интерпретатора с исходного RIP → тесты 3 и 5 (счётчик устройства 2).
  - Вспомогательный рефакторинг теста: заполнение памяти `make_fetch_bus` вынесено в `populate_fetch_memory` для переиспользования MMIO-шиной; в нём же вторая инструкция `FOLLOWING_CODE` (`48 89 0B`) по `CODE_PHYSICAL + 3`.
- **Результаты.** Команды и коды — в итоговом отчёте исполнителя; `cargo test -p od-host --offline --test stage4_memory_fault` — 14 passed (7 прежних + 7 новых).
- **НЕ сделано.** JIT store (S4-15d), расширение допустимых форм и многоинструкционные блоки (S4-15e), reset/retirement и live handle (S4-15f), закрытие S4-15 и обновление контракта целиком (S4-15g); 2 MiB код-страницы; MMIO через JIT (MMIO по-прежнему только интерпретатором); интеграция диспетчера в runner/Linux-трассу; перформанс.
- **Риски.** Fetch-walk делается дважды при fallback после fetch (второй раз без commit-ов; цена не измерялась); один MAP_JIT регион — `Publish` отказывает при живом блоке другой сессии, диспетчер в этом случае тихо идёт в интерпретатор (виден в `reason`); классификация «MemoryError ⇒ нет effects» опирается на проверку `unchanged()` при декодировании и на RAM-only адаптер, который не вызывает устройства; при смене адаптера на MMIO-capable это допущение надо пересмотреть.
- **Ревью (Claude Opus, high): принято с ограничениями.** P2-1 закрыт тестом «лишний шаг интерпретатора» (вторая инструкция `48 89 0B` в фикстуре, мутация проверена). P3 закрыты: полное сравнение состояния (`LongModeState` целиком), проверка `PageFaultRecord`, явное сопоставление исходов JIT (`Complete`/`GuestFault`), тест `Publish(MappingAlreadyLive)` (тест 7). Ограничение: ветки `JitHostFailure` тестом не покрыты.

## S4-15d — JIT-store `48 89 03` на production owner через tracked store-адаптер (реализовано; ревью Claude Opus, high — принято с ограничениями; итог S4-15 — в конце раздела)

> **Заменено S4-16i.** Отказ `ActiveCodePage` (store в физическую страницу выполняющегося блока, п. 3 и 5 правила store, тест 6, мутация M1) описывает поведение S4-15d, а не итоговое: S4-16i его убрал. Store в активную страницу теперь разрешён, коммитится и завершается выходом `CodeInvalidated` (см. раздел S4-16i в конце отчёта). Остальное в этом разделе (NonRamTarget, ProbeUnsupported, #PF до записи, неактивные страницы) остаётся в силе.

- **Зачем.** После S4-15c JIT на production owner обслуживал только загрузку `48 8B 03`; store шёл только в интерпретатор (а `48 89 03` интерпретатор не исполняет вовсе: его allowlist — `48 8B 03` и `48 89 0B`). Адаптер загрузки `with_paged_mov_memory` работает с `read_only_qword: true` (запись отказывает), а `unsafe`-контракт `execute_memory_store_v2` («private synchronous RAM, не executable, без alias») для production owner нужно было обосновать отдельно.
- **Что сделано** (существующие пути S4-15a/b/c и интерпретатор не менялись; `PagedFetchedLoadSession::execute` по-прежнему допускает только load и отдаёт `UnsupportedForm` для `48 89 03` — в том числе при повторном входе на сессии, где store-блок с теми же байтами уже допущен через `execute_with_store`: ветка `Reused` проверяет форму до запуска, блок остаётся допущенным).
  - `crates/od-machine/src/paged_memory_adapter.rs`: `with_paged_mov_store_memory(bus, paging, rip, active_code_page, cb) -> PagedMovStoreScope<R>`, `PagedStoreRefusal::{NonRamTarget{physical}, ActiveCodePage{page}, ProbeUnsupported}`. Отдельный маленький `GuestMemory` (`PagedMovStoreMemory`), который умеет только `write` ровно 8 байт внутри 4 KiB-страницы; fetch/read/atomics/mfence/другие ширины/cross-page — типизированная backend-ошибка. Запись идёт через существующий tracked-путь `MachineBus::access_tracked_ram` (page walk: Present, Write (CR0.WP на CPL0), User (CPL3), reserved; NX на запись не влияет; затем A-биты, D-бит листа и только потом байты операнда, всё через callback committed-writes; `PhysicalMemory::write_bytes` проверяет весь диапазон до коммита).
  - `crates/od-host/src/lib.rs`: `PAGED_FETCHED_STORE_V2_BYTES`; `PublishedCode::execute_paged_mov_rbx_rax_store_v2(state, bus, cr3, efer, ctx, active_code_page)` (`pub(crate)`: единственный вызывающий — `PagedFetchedLoadSession`, он берёт `active_code_page` из tracked fetch-снимка; внешних вызывающих в репозитории нет; SAFETY-комментарий переписан: безопасность памяти от `active_code_page` не зависит — блок в MAP_JIT, RAM — отдельная куча, аргумент влияет только на отказ SMC; строгая проверка метаданных: flavor `S3_20bMemoryStoreV2`, байты `48 89 03`, источник RAX, база RBX, без индекса/смещения; контекст CPU, CR3, operand range, paging — как у load; `unsafe`-вызов `execute_memory_store_v2` с развёрнутым SAFETY-комментарием); `PagedFetchedLoadSession::execute_with_store` (JIT-only вход: load или store с тем же fetch-снимком и ключом повторного допуска (RIP, физ. страница, generation, байты) — `execute` и `execute_or_interpret` делят `execute_form`); `PagedFetchedLoadReport::store_refusal`; `PagedStepFallbackReason::StoreTargetRefused(PagedStoreRefusal)`; `execute_or_interpret` теперь вызывает `execute_with_store`.
- **Правило store (порядок).**
  1. Контекст CPU, CR3 CPU-owned, operand range (операнд внутри одной 4 KiB-страницы; иначе `Load(UnsupportedOperandRange)` до любого walk), paging-контролы — отказ без effects.
  2. Tracked fetch 3 байт по RIP (execute/NX/U/S, A-биты кода коммитятся), снимок: байты, физ. страница, generation. Байты `48 89 03` → допуск/повторное использование блока по ключу; иные байты — `UnsupportedForm`, блок сброшен.
  3. В адаптере, ДО любого walk с коммитом: read-only probe трансляции операнда (`probe_tracked_ram_access`, без A/D). Если трансляция успешна и операнд не целиком обычная RAM (MMIO, ROM, framebuffer, unmapped) → `NonRamTarget`; [S4-15d: если операнд на физической странице, с которой выбран сам блок → `ActiveCodePage` — заменено в S4-16i: такого отказа больше нет]. Отказ — ничего не записано, ни A/D, ни устройство; helper оставляет CPU state неизменным → `StoreTargetRefused` → один вызов `step_paged_mov` по правилу S4-15c. Если probe вернул ЛЮБУЮ ошибку (2 MiB/1 GiB лист — `UnsupportedPageSize`, не-canonical адрес, нечитаемая таблица) → `ProbeUnsupported`: так же отказ до walk, ни A, ни D, ни данных; раньше такая ошибка уходила в `access_tracked_ram`, который коммитил A на верхних уровнях и возвращал backend-ошибку, а интерпретатор (без data-walk) расходился с pure-интерпретатором по списку writes и образу RAM. Если probe нашёл #PF — решение не принимается здесь.
  4. Tracked store: walk (A-биты) → при отказе прав guest #PF ДО D-бита и до записи байт (`GuestFault`, `PageFaultRecord` из walker, A-биты остаются) → D-бит листа → запись 8 байт → callback committed-writes. #PF — исполненный результат JIT и интерпретатором не повторяется (как data #PF в S4-15c).
  5. Блок и страницы: store в НЕАКТИВНУЮ code page разрешён — сессия держит один блок и перед каждым входом заново делает tracked fetch и сверяет (RIP, страница, generation, байты), так что старые байты не выполняются; это аналог `reconcile_code_writes` для данной сессии (alias-кэшей и `PagedCodeSession` здесь нет). Store в собственную активную страницу — в S4-15d отказ до commit (п. 3); заменено в S4-16i (разрешён, выход `CodeInvalidated`).
- **Важное отклонение/факт.** Интерпретатор `step_paged_mov` не имеет `48 89 03` (Unsupported opcode 0x89, 0 retired). Поэтому (а) oracle для исполненного JIT-store — тот же store в интерпретаторе в форме `48 89 0B` (MOV [RBX],RCX) при RCX=RAX на идентичной RAM (байт кода в oracle-RAM восстанавливается перед сравнением образа); (б) fallback после отказа store (MMIO, собственная страница) даёт `RunExit::Unsupported`, store не исполняется ни одним движком — как и до S4-15d для любой `48 89 03`; (в) пункт задания «(б) fault → интерпретатор» реализован по правилу S4-15c: #PF — финальный результат JIT (`PagedStepPath::Jit`, равенство oracle), в интерпретатор уходят только отказы без effects. Причина и адресат остатка — в разделе «S4-15 — итог» (ограничения): Нет в одношаговом paged-профиле `step_paged_mov` (allowlist `is_paged_mov_instruction`: `48 8B 03` и `48 89 0B`); формы `48 89 03` и `8B 03` исполняются в od-isa (плоский s2-интерпретатор, dword-профиль `step_paged_mov_dword_read`, paged CPL0-набор на плате), но fallback к ним из диспетчера не подключён. Slice с гарантией fallback — `48 8B 03` (и `48 89 0B` исполняет интерпретатор); `48 89 03` — расширение JIT БЕЗ гарантии fallback при отказе (отказ явный, `Unsupported`, без молчаливого effect). Адресат остатка: S4-42 (fallback на paged CPL0-набор на Linux-трассе) либо отдельный подсрез od-isa/CPU_CONTRACT, если нужен одношаговый paged-профиль с `48 89 03` (вне «Где» S4-15).
- **Тесты** (`tests/system/stage4_memory_fault.rs`, 8 новых; сравниваются 16 GPR, RIP, весь `LongModeState`, `PageFaultRecord`, упорядоченные committed writes, образ RAM):
  1. `s4_15d_store_on_ram_equals_the_interpreter_and_sets_the_dirty_bit` — CPL0 user, CPL3 user, CPL0 supervisor leaf, NX data leaf (NX не влияет на запись), лист с предустановленным D (одна запись листа вместо двух): равенство oracle, последняя запись `(DATA_QWORD, 8)`, D только на листе, A на всех 8 уровнях, повторный вход — `Reused`. Мутации: «адаптер читает вместо записи» (M5) → падает; лишний интерпретаторский шаг после JIT (следующая инструкция фикстуры) виден по RIP/RAM/oracle.
  2. `s4_15d_store_faults_precede_the_write_and_equal_the_interpreter` — not-present и read-only (CPL0 при WP, CPL3): PFEC 0x2/0x3/0x7/0x6, CR2, saved RIP, access Write; данные не изменены, D нигде нет, в RAM изменились только A-биты таблиц, операнд не среди committed writes. Мутация M5 → падает.
  3. `s4_15d_cpl3_store_to_a_supervisor_page_faults_before_the_write` — CPL3 + U/S=0: PFEC 0x7, запись не произошла; JIT-only вход: блок остаётся допущенным; `execute` (S4-15b) всё ещё отдаёт `UnsupportedForm` для `48 89 03`. Мутация M3 «CPL3 как CPL0» → падает (вместе с тестом 2).
  4. `s4_15d_mmio_store_is_refused_before_any_effect_and_not_executed` — JIT-only: `NonRamTarget{MMIO_BASE+0x120}`, state неизменён, 0 committed writes, 0 вызовов устройства, A-биты data-ветки не установлены (probe-отказ до walk); диспетчер: путь Interpreter, `StoreTargetRefused`, `Unsupported`, равенство pure-interpreter oracle на тех же байтах (любой просочившийся effect — A/D или запись устройства — разошёлся бы), writes = только code-walk. Мутация M2 (убрана проверка не-RAM) → падает.
  5. `s4_15d_store_into_another_code_page_makes_the_old_bytes_unreachable` — load на странице 0xa000 исполнен и допущен; store-блок на странице 0x8000 переписывает байты load в `48 89 03`; повторный запуск store — `Reused` (запись в другую code page не инвалидирует работающий блок, generation его страницы не менялась); вход по адресу переписанного кода — `Admitted` заново из новых байтов и generation (store, а не load: RAX = marker, data qword = marker; старый load загрузил бы RAX из памяти). Admissions = 3. Проверяет также рост generation записанной страницы.
  6. [заменён в S4-16i тестами `s4_16i_*`; ниже описание S4-15d] `s4_15d_store_into_the_executing_code_page_is_refused_before_commit` — операнд на странице блока рядом с байтами (offset 0x120) и поверх них (offset 0, NOP-ы): JIT-only — `ActiveCodePage`, state неизменён, 0 writes, A data-ветки нет, страница кода не изменена; диспетчер — путь Interpreter, `StoreTargetRefused`, равенство oracle, страница не изменена; повторный вход — блок `Reused`, тот же отказ, RAM не меняется. Мутация M1 (убрана проверка активной страницы) → падает.
  7. `s4_15d_load_only_entry_never_runs_a_store_block_admitted_by_the_store_entry` (по ревью, P2-1) — `execute_with_store` допускает и исполняет store; затем `execute()` на той же сессии с теми же байтами и generation → `UnsupportedForm{48 89 03}`, `LongModeState` неизменён (RIP, RAX), данные не записаны, образ RAM равен, `admissions == 1`; затем `execute_with_store` снова даёт `Reused` и пишет (отказ не «липкий»).
  8. `s4_15d_store_with_an_untranslatable_operand_is_refused_without_any_data_walk_effect` (по ревью, P2-2) — data PDE `0x87` (2 MiB, PS) по адресу 0x5000: JIT-only — `store_refusal == ProbeUnsupported`, state неизменён, 0 committed writes/AD-updates, на data-ветке нет ни A, ни D, в RAM изменились только A-биты code-walk; диспетчер — путь Interpreter, `StoreTargetRefused(ProbeUnsupported)`, `Unsupported`, равенство pure-interpreter oracle (reports, writes = только code-walk, образ RAM). Случай не-canonical RBX в тест не включён: он не доходит до адаптера (`store_refusal == None`), его обрабатывает более ранний слой.
  - Мутации (вручную, затем откат из копии, `cmp` после отката): M1 — тест 6; M2 — тест 4; M3 — тесты 2 и 3; M5 — тесты 1, 2, 3, 5. По ревью воспроизведена и мутация (б) «коммит до проверки прав»: её ловят `s4_15d_store_faults_precede_the_write_and_equal_the_interpreter`, `s4_15d_cpl3_store_to_a_supervisor_page_faults_before_the_write` и `s4_15d_store_into_the_executing_code_page_is_refused_before_commit`. Мутации правок по ревью: P2-1 (условие в ветке `Reused` заменено на `false &&`) → падает тест 7 (`execute()` вместо `UnsupportedForm` исполнил store), `cmp` после отката — идентичен; P2-2 (`Err(_) => {}` вместо отказа, т.е. прежнее поведение) → падает тест 8 (`store_refusal` None вместо `ProbeUnsupported`), `cmp` после отката — идентичен.
- **Результаты.** `cargo fmt --all -- --check` — расхождения только в `storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs` (уже в main); `cargo check --workspace --all-targets --offline` — 0; `cargo test -p od-host --offline --test stage4_memory_fault` — 22 passed (14 + 8); `cargo test -p od-host --offline --lib s4_15` — 1 passed; `cargo test -p od-jit --offline --lib` — 455 passed, 2 ignored.
- **НЕ сделано.** Store в активный блок и инвалидация активного блока (сделано позже в S4-16i); расширение допустимых форм и многоинструкционные блоки (S4-15e); reset/retirement и live handle (S4-15f); закрытие S4-15 и обновление контракта целиком (S4-15g); store в MMIO/framebuffer через JIT и исполнение refused-store интерпретатором (адресат остатка — S4-42 либо подсрез od-isa/CPU_CONTRACT; см. «S4-15 — итог»); 2 MiB код-страницы; cross-page операнд; запись в страницу таблиц, участвующих в walk (не отказывается отдельно); интеграция в runner/Linux-трассу; перформанс.
- **Риски.** Probe + tracked access — два трансляционных прохода на store (цена не измерялась); probe и walker читают одни таблицы, но решение о RAM/активной странице принимает probe — если таблицы меняются самим операндом (alias с таблицей walk), walker остаётся источником истины для результата, а отказ может быть консервативным или пропущенным только в патологическом self-referencing случае; framebuffer-регион отказывается (`NonRamTarget`) вопреки тому, что walker интерпретатора пишет в него как в RAM; один MAP_JIT регион; `JitHostFailure` для store не покрыт тестом по той же причине, что в S4-15c.
- **Ревью (Claude Opus, high): принято с ограничениями.** P2-1 закрыт тестом 7 (`s4_15d_load_only_entry_never_runs_a_store_block_admitted_by_the_store_entry`: load-вход не исполняет store-блок, допущенный store-входом); P2-2 закрыт тестом 8 (`s4_15d_store_with_an_untranslatable_operand_is_refused_without_any_data_walk_effect`: ошибка probe — отказ без записи и без A/D). P3 закрыты: исправлен текст про мутацию, store-обёртка `pub(crate)`.

## S4-15e — расширение допуска JIT-форм на production owner: dword, disp8/disp32, RIP-relative (реализовано; ревью Claude Opus, high — принято с ограничениями; итог S4-15 — в конце раздела)

- **Зачем.** После S4-15d JIT допускал ровно `48 8B 03` и `48 89 03`. Срез добавляет операндные формы через тот же tracked RAM-адаптер и тот же fetch-снимок (RIP, физ. страница, generation, байты), не меняя пути S4-15a–d.
- **Что сделано** (старые входы `execute`, `execute_with_store`, `execute_or_interpret` и их поведение не менялись: они по-прежнему допускают только `48 8B 03` / `48 89 03`, остальное — `UnsupportedForm` по трём байтам, operand range проверяется до fetch).
  - `crates/od-machine/src/paged_memory_adapter.rs`: `with_paged_mov_dword_memory` (экспорт из `od-machine`) — тот же RAM-only/read-only адаптер, что `with_paged_mov_memory`, но с четырёхбайтовым чтением (через общий приватный `with_paged_mov_read_memory`; флаг `cpl0_set_dword_read` без `large_ram_profile` меняет только допустимую ширину чтения, путь остаётся обычным `access_tracked_ram`, A-биты попадают в `committed_writes`). Отказ cross-page чтения у адаптера распространён с qword на dword (для qword поведение прежнее).
  - `crates/od-host/src/lib.rs`: `PagedFetchedLoadSession::execute_forms` + `PagedFetchedForm` + `PagedFetchedFormReport`; `PublishedCode::execute_paged_checked_load_v2` (`pub(crate)`). Длина инструкции не фиксирована: tracked fetch сначала 2 байта (форма `8B 03` закончена), иначе 3, затем 4 или 7 байт, если опкод требует (A-биты кода коммитятся первым снимком, повторные идемпотентны). Код, не помещающийся в 4 KiB-страницу RIP, — `Fetch(InvalidLength)`, а не #PF. Operand range считается ПОСЛЕ fetch от эффективного адреса формы (`UnsupportedOperandRange{address, length}`, A-биты fetch остаются в `fetch_committed_writes` ошибки). Эффективный адрес = база (RBX либо RIP СЛЕДУЮЩЕЙ инструкции) + знаково расширенное смещение, считается на каждом вызове из `LongModeState`, в блоке не хранится. Host-исполнитель сверяет метаданные опубликованного блока (flavor, байты, операция, RAX, база/смещение/размер операнда) и независимо сверяет адрес декодера (`MemoryOperand::effective_address`) с адресом формы; расхождение — `UnsupportedBlock`.
  - Компиляция: dword — `compile_ir_memory_dword_load_block_v2` → `publish_ir_memory_dword_load_v2` (flavor `S3_26c`); qword-load со смещением/RIP-relative — `compile_ir_memory_load_block_v2` → `publish_ir_memory_load_v2` (flavor `S3_26a`); `48 8B 03`/`48 89 03` — прежние S3_19b/S3_20b. `od-jit` и `od-isa` не менялись.
- **Таблица покрытия форм.**

| Форма | Байты | Операнд | Допущена JIT (`execute_forms`) | Интерпретатор (paged allowlist) | Тест |
|---|---|---|---|---|---|
| MOV RAX,[RBX] | `48 8B 03` | qword | да (S4-15b) | да `step_paged_mov` | `s4_15e_the_existing_forms_through_execute_forms_equal_the_interpreter` |
| MOV [RBX],RAX | `48 89 03` | qword | да (S4-15d) | нет (есть `48 89 0B`, store с RCX) | тот же тест (oracle `48 89 0B`, RCX=RAX) |
| MOV EAX,[RBX] | `8B 03` | dword, zero-extend в RAX | да | да, отдельный профиль `step_paged_mov_dword_read` | `s4_15e_dword_load_zero_extends_*`, `s4_15e_dword_load_may_end_exactly_*`, `s4_15e_dword_load_at_the_end_of_the_code_page_*` |
| MOV RAX,[RBX+disp8] | `48 8B 43 ib` | qword | да | нет | `s4_15e_qword_displacement_loads_*` |
| MOV RAX,[RBX+disp32] | `48 8B 83 id` | qword | да | нет | `s4_15e_qword_displacement_loads_*` |
| MOV RAX,[RIP+disp32] | `48 8B 05 id` | qword, от RIP следующей инструкции | да | нет | `s4_15e_rip_relative_load_*`, `s4_15e_reuse_key_keeps_rip_*` |
| addr32 `67 8B 03` и др. `67`-формы | `67 ...` | — | нет (`UnsupportedForm`) | нет | `s4_15e_forms_outside_the_allowlist_*` |
| store со смещением | `48 89 43 ib`, `48 89 83 id` | — | нет (`UnsupportedForm`) | нет | `s4_15e_forms_outside_the_allowlist_*` (`48 89 43 08`) |
| dword со смещением | `8B 43 ib` и т. п. | — | нет (`UnsupportedForm`) | нет | `s4_15e_forms_outside_the_allowlist_*` |

- **Важное отклонение/факт: oracle.** Интерпретатор (`step_paged_mov`, dword-профиль) не имеет ни смещения, ни RIP-relative, ни `67`, ни store со смещением (`is_paged_mov_instruction` требует `displacement == 0`, `length == 3`, REX ровно `0x48`). Поэтому: dword сравнивается с настоящим `step_paged_mov_dword_read`; disp8/disp32/RIP-relative — с эквивалентной инструкцией интерпретатора (`48 8B 03` на идентичной RAM, RBX = эффективный адрес, который тест считает независимо от RIP, RBX и смещения; RBX и RIP oracle нормализуются, байты кода в oracle возвращаются перед сравнением образа RAM) — тот же приём, что в S4-15d для store. Store со смещением и `67` не допущены: интерпретатор их не имеет, и дальше расширять «своим авторитетом JIT» не стали. Они ждут формы в `od-isa`.
- **Ключ допуска.** Без изменений: (RIP, физическая страница, generation, байты). RIP входит в ключ как точный `linear_base`, поэтому RIP-relative блок не переиспользуется на другом RIP (иначе `EntryRipMismatch`); адрес операнда каждый раз считается заново из RIP и RBX.
- **Тесты** (`tests/system/stage4_memory_fault.rs`, 13 новых `s4_15e_*`; параметры сравнения с oracle: exit, retired, `PageFaultRecord`, упорядоченный список committed writes, весь образ RAM, полный `LongModeState`):
  1. `s4_15e_dword_load_zero_extends_into_rax_and_equals_the_dword_interpreter` — CPL0, CPL3, not-present лист данных (PFEC 0), supervisor-лист при CPL3 (PFEC 5); RAX = только младшие 32 бита, старшие 0 при `INITIAL_RAX` со старшей половиной ≠ 0.
  2. `s4_15e_dword_load_may_end_exactly_at_a_page_boundary_but_not_cross_it` — dword на смещении 0xffc читается (qword был бы отказан), на 0xffd — `UnsupportedOperandRange{length: 4}` без data walk.
  3. `s4_15e_dword_load_at_the_end_of_the_code_page_needs_only_its_two_bytes` — `8B 03` на RIP offset 0xffe при следующей странице NX: хватает двух байт fetch.
  4. `s4_15e_qword_displacement_loads_...` — disp8 +/−, disp32 +/−, CPL0/CPL3, not-present, supervisor/CPL3; CR2 = эффективный адрес, RBX не меняется.
  5. `s4_15e_rip_relative_load_is_relative_to_the_next_instruction_not_rbx_or_the_start` — данные во второй code page (RBX указывает на другие читаемые данные), успех/CPL3/#PF; walk касается только листа 0x6010.
  6. `s4_15e_reuse_key_keeps_rip_...` — те же байты, та же физстраница и generation, RIP +0x40: `Admitted` заново и значение по адресу от нового next RIP; повторный вход на том же RIP — `Reused` и свежие данные.
  7. `s4_15e_reused_displacement_block_recomputes_the_address_from_rbx`.
  8. `s4_15e_effective_address_crossing_a_page_is_refused_before_the_data_walk` — disp8, disp32 (RBX сам проходит проверку, база+смещение — нет), RIP-relative; A-биты fetch в ошибке, data walk нет.
  9. `s4_15e_fetch_faults_of_the_longer_forms_...` — dword/disp8/disp32/RIP-relative × NX, supervisor/CPL3, not-present: `PageFaultRecord` равен записи `step_paged_mov`, writes и RAM равны, data walk нет.
  10. `s4_15e_code_that_does_not_fit_the_page_is_refused_not_faulted` — `48 8B 83` на offset 0xffd: `Fetch(InvalidLength)`.
  11. `s4_15e_forms_outside_the_allowlist_are_unsupported_forms_with_no_effect` — store disp8, `67 8B 03`, dword disp8, `48 8B 0B`, NOP: `UnsupportedForm`, state неизменён, прежний блок сброшен.
  12. `s4_15e_legacy_entries_do_not_admit_the_new_forms` — `execute`/`execute_with_store` отказывают dword/disp8/RIP-relative.
  13. `s4_15e_the_existing_forms_through_execute_forms_equal_the_interpreter` — `48 8B 03` и `48 89 03` через новый вход.
  - Мутации (вручную, затем откат из копии, `cmp` после отката — идентично): dword не зануляет старшие 32 бита (`runtime.rs`, staged value) → падают тесты 1, 2, 3; RIP-relative от RIP начала инструкции → тесты 5, 6, 8; disp8 как беззнаковое → тест 4; operand range от RBX вместо эффективного адреса → тест 8; ключ повторного допуска без RIP → тест 6; dword через qword-адаптер → тесты 1, 2, 3; dword operand 8 байт → тест 2; не сбрасывать блок при `UnsupportedForm` → тест 11; fetch всегда 7 байт → тесты 1–4, 7, 8, 11, 13; `48 8B 43` объявлен неподдерживаемым → тесты 4, 7, 8, 11.
- **Результаты.** `cargo fmt --all -- --check` — расхождения только в `storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs` (уже в main); `cargo check --workspace --all-targets --offline` — 0; `cargo test -p od-host --offline --test stage4_memory_fault` — 35 passed (22 + 13); `cargo test -p od-host --offline --lib s4_15` — 1 passed; `cargo test -p od-jit --offline --lib` — 455 passed, 2 ignored; дополнительно `cargo test -p od-machine --offline` — 1382 passed в основном наборе (изменён общий адаптер).
- **НЕ сделано.** Формы вне allowlist `step_paged_mov` (store со смещением, `67`-формы, dword со смещением, другие регистры), хотя `od-jit` умеет компилировать часть из них; интеграция новых форм в `execute_or_interpret` (интерпретатор не исполняет их, поэтому fallback закончился бы `Unsupported`; dword мог бы падать на `step_paged_mov_dword_read`, но выбор интерпретатора по форме не сделан); многоинструкционные блоки; 2 MiB-страницы кода; код через границу 4 KiB-страницы; закрытие S4-15 и обновление контракта целиком (S4-15g); reset/retirement (S4-15f).
- **Риски.** Эквивалентный oracle для disp/RIP-relative слабее настоящего (общий `step_paged_mov` проверяет доступ и #PF по адресу, но не декодирование смещения интерпретатором: оно проверено независимым вычислением адреса в тесте и сверкой с декодером в host-исполнителе); fetch делает до трёх tracked-снимков на допуск (A-биты идемпотентны, цена не измерялась); новый вход проверяет operand range после fetch, поэтому отказ уже закоммитил A-биты кода (в отличие от старых входов); один MAP_JIT регион.
- **Ревью (Claude Opus, high): принято с ограничениями.** P3 закрыт: исправлен текст комментария адаптера. Остаются записанные ограничения среза: интеграция `execute_forms` в `execute_or_interpret`, формы вне allowlist, эквивалентный oracle для disp/RIP-relative (см. «НЕ сделано» и «Риски» выше).

## S4-15f — reset и освобождение MAP_JIT-региона на production owner (реализовано; ревью Claude Opus, high — принято; итог S4-15 — в конце раздела)

- **Зачем.** После S4-15e у JIT-сессии не было определения reset, а освобождение executable-региона на production owner не проверялось (lib-тесты арены смотрели `resident_entries()`/`available_slots()` только на фикстурах кэша). Срез определяет reset, делает освобождение наблюдаемым и закрывает путь Drop арены при занятом borrow. Один vCPU, синхронно; поведение путей S4-15a–e не менялось.
- **Что именно владеет блоком (поправка к исходной формулировке «слот арены»).** `PagedFetchedLoadSession` держит блок как `PublishedCode`, то есть единственный на процесс MAP_JIT-регион (`MapJitLease` + `munmap`), а не слот `PublishedArenaBackend`. Арена и сессия взаимно исключают друг друга (общий lease). Поэтому «слот» сессии — это сам регион: он один, занят (0 свободных) от допуска до reset/Drop/инвалидации и свободен иначе. Слоты арены (`available_slots`) проверены отдельно на lib-уровне.
- **Что сделано.**
  - `crates/od-host/src/lib.rs`, `PagedFetchedLoadSession::reset(&mut self) -> bool` и `resets()` (новое поле `resets`; `admissions` кумулятивен и reset переживает): `admitted.take()` роняет `PublishedCode` — `munmap` и lease освобождены. Возвращает `true`, если блок был; повторный reset без блока — no-op (`false`), освободить регион дважды или чужой регион нельзя. Состояние гостя и RAM reset не трогает. Следующий вход допускает форму заново (`Admitted`, не `Reused`).
  - Safe point: `reset` берёт `&mut self`, как и все входы исполнения, поэтому borrow checker запрещает reset изнутри исполняемого блока; блок вызова живёт до возврата вызова и освобождается только reset, инвалидацией (следующий вызов роняет устаревший блок до компиляции) или Drop сессии.
  - `pub fn map_jit_region_live() -> bool` — наблюдение lease (на не-macOS/arm64 всегда `false`).
  - Путь Drop арены при занятом borrow (`PublishedArenaBlock::drop`; раньше неудача `try_borrow_mut` молча выходила, слот тёк навсегда). Воспроизводим синхронно в одном потоке: `execute_tracked_code_store_*` держит общий `arena.borrow()` на всё время preflight владельца и нативного вызова, а `TrackedExecutableCodeOwner::preflight_code_store` (код владельца) может уронить другой handle арены. Теперь при неудаче `try_borrow_mut` handle записывает своё поколение в `ArenaSlot::pending_release: Cell<u64>` (достаточно общего borrow); отложенный release применяется (`drain_pending_releases`) при следующем изменяемом доступе — Drop другого handle и все три `publish_*` — до выбора свободного слота. `available_slots()` считает отложенные слоты свободными, `cached_block_capacity()` не считает их занятыми, а метаданные слота остаются нетронутыми, пока идёт вызов (слот не освобождается изнутри блока). Общая логика вынесена в `ArenaInner::release_slot` (проверка поколения и «слот занят»: повторный release слота — no-op, двойной push в `free_slots` невозможен). Остаточный случай: если и `try_borrow` неудачен (активный `borrow_mut`), слот не освобождается, но `borrow_mut` арены не держится ни через один callback, поэтому из синхронного кода это недостижимо.
- **Тесты.**
  - Интеграционные (`tests/system/stage4_memory_fault.rs`, +5, итого 40). Число свободных слотов — `jit_free_slots()`: пробная арена на 16 слотов создаётся и показывает 16 свободных только при свободном регионе, иначе `MappingAlreadyLive`; результат сверяется с `map_jit_region_live()`.
    1. `s4_15f_reset_between_two_entries_readmits_and_equals_the_interpreter` (а) — вход, reset, вход: оба `Admitted`; наблюдение (GPR, RIP, RFLAGS, writes, образ RAM) равно двум проходам интерпретатора на своей RAM; контроль: без reset следующий вход — `Reused`.
    2. `s4_15f_free_slots_return_to_the_initial_count_after_reset_and_after_drop` (б) — 16 → 0 после допуска → 16 после reset → 0 → 16 после Drop сессии; то же для store-формы; reset без блока — `false`.
    3. `s4_15f_the_block_stays_owned_after_its_execution_until_reset_or_drop` (в) — после успешного и после faulting вызова блок и регион остаются занятыми (повторный вход `Reused`), освобождаются только reset.
    4. `s4_15f_repeated_reset_and_late_drop_never_release_another_sessions_region` (г) — сессия B не может опубликовать, пока регион у A (`Publish(MappingAlreadyLive)`); после reset A B допускает; повторный reset и Drop A не освобождают регион B (`map_jit_region_live()` остаётся `true`, B исполняется и `Reused`); reset B, второй reset — no-op.
    5. `s4_15f_generation_invalidation_replaces_the_block_without_leaking_the_region` (д) — 5 циклов «переписать байт, `Admitted`» без роста занятости (всегда ровно один регион); патч в байты вне allowlist роняет блок и освобождает регион без reset; восстановление — `Admitted`; reset — свободно.
  - Lib-тесты (`crates/od-host/src/lib.rs`, +3, `s4_15f_*`, арена):
    6. `s4_15f_arena_release_is_exact_and_a_duplicate_release_is_a_no_op` (г, арена) — подделанный второй handle того же слота и поколения: второй Drop и прямой `release_slot` свободного слота не меняют счёт и стек; 40 циклов publish/Drop без утечки, поколения растут.
    7. `s4_15f_handle_dropped_under_a_borrow_is_deferred_not_leaked` — Drop handle под удерживаемым `arena.borrow()`: метаданные нетронуты, стек пуст, один отложенный; после borrow `available_slots()==1`, следующий publish успешен (не `CapacityFull`), итог 2/2.
    8. `s4_15f_handle_dropped_by_the_owner_during_a_native_call_is_freed_after_return` (в, арена) — реальный сценарий: владелец (фикстура `TrackedCodeFixture`, новое поле `drop_in_preflight`) роняет другой handle в `preflight_code_store` во время нативного store; изнутри вызова наблюдается (0 свободных, 1 отложенный, метаданные целы), после возврата `available_slots()==1`, слот переиспользуется.
  - Мутации (вручную, откат из копии, `cmp` после каждого отката — идентично): reset не роняет блок (`is_some` вместо `take`) → падают все 5 интеграционных; reset всегда `true` и всегда считает → 2, 4, 5; reset безусловно сбрасывает глобальный lease → 4; Drop сессии «забывает» блок (`mem::forget`) → 2, 4, 5; reuse без сверки generation, и отдельно устаревший блок не сбрасывается в `execute` → 5; busy-Drop не записывает отложенный release (поведение до S4-15f) → 7, 8; убран guard «слот занят» в `release_slot` → 6; `available_slots()` без отложенных → 7, 8; убран `drain_pending_releases` из `publish_*` → 7, 8.
- **Результаты.** `cargo fmt --all -- --check` — расхождения только в `storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs` (уже в main); `cargo check --workspace --all-targets --offline` — 0; `cargo test -p od-host --offline --test stage4_memory_fault` — 40 passed (35 + 5), код 0; `cargo test -p od-host --offline --lib s4_15` — 4 passed (1 + 3), код 0; полный `cargo test -p od-host --offline --lib` — 382 passed, код 0; `cargo test -p od-jit --offline --lib` — 455 passed, 2 ignored, код 0.
- **НЕ сделано.** SMP и отложенное освобождение между vCPU (S3-25, G7): reset и Drop предполагают один vCPU и отсутствие исполнения в момент освобождения; освобождение «после допустимого срока» (grace period) не реализовано — освобождение немедленное на safe point. Инвалидация активного блока (S4-16i), интеграция reset в `execute_or_interpret`/runner (reset вызывается только явно), закрытие S4-15 и обновление контракта целиком (S4-15g). Наблюдение «регион занят во время исполнения блока» изнутри блока сессии невозможно (data-операнд идёт только в RAM, callback из блока нет): доказательство — `&mut self` и состояние после возврата; внутри-callback случай проверен только для арены (тест 8).
- **Риски.** Отложенный release арены освобождает слот лениво (при следующем `publish_*` или Drop другого handle; `available_slots()` только учитывает отложенные слоты и ничего не освобождает), поэтому прямые чтения `free_slots` вне этих мест не видят отложенные слоты; единственный остаточный путь утечки — Drop при активном `borrow_mut`, которого сейчас нет ни в одном callback. `available_slots()` теперь включает отложенные слоты (без callback-Drop значение прежнее). Тест (г) для сессии доказывает отсутствие двойного/чужого освобождения на уровне lease, но не снимает ограничения «один регион на процесс». `map_jit_region_live()` — глобальный флаг процесса: тесты, использующие его, должны сериализоваться (они берут `JIT_TEST_LOCK`).
- **Ревью (Claude Opus, high): принято.** Замечание P3 закрыто: исправлен текст про `available_slots()` (он только учитывает отложенные слоты и ничего не освобождает). Записаны как ограничения (не закрыты тестами): ветки `Ok` в `drain_pending_releases` не покрыты тестом; проверка занятости слота в ветке `Err` отложенного release не покрыта тестом.

## S4-15 — итог (закрытие, S4-15g; gate-ревью Claude Opus, high: принять с записанными ограничениями)

Документальный срез: код не менялся. Контракт целиком — раздел «S4-15: JIT on the production memory owner (closure, S4-15g)» в [JIT_CONTRACT.md](../specs/JIT_CONTRACT.md). Срезы S4-15a–f приняты по отдельности (a — принят; b, c, d, e — принято с ограничениями; f — принято, ревью Claude Opus, high); здесь они сведены к пунктам плана `docs/plans/STAGE_4.md`, S4-15. Предложен к приёмке с ограничениями: gate-ревью Claude Opus (high) — принять с записанными ограничениями; чекбокс S4-15 в плане проставляется координатором.

Обозначения: в разделах срезов S4-15b–f номера тестов — порядок теста в `tests/system/stage4_memory_fault.rs` (для lib-тестов S4-15f — продолжение нумерации интеграционных тестов в том же блоке отчёта: 6–8). В таблице итога вместо номеров приведены полные имена тестов (проверены по `grep -n "fn s4_15"` в `tests/system/stage4_memory_fault.rs` и `crates/od-host/src/lib.rs`).

### Пункты плана и чем они покрыты

Тесты названы полными именами (все в `tests/system/stage4_memory_fault.rs`, если не указано `lib`); `*` в конце имени сокращает хвост.

| Пункт плана | Чем покрыт (срез, тесты) |
|---|---|
| Где: `od-jit`, `od-machine`, `od-host` в существующих границах; обновление `JIT_CONTRACT.md` | Сессия и входы в `od-host`, адаптеры в `od-machine` (`with_paged_mov_memory`, `with_paged_mov_dword_memory`, `with_paged_mov_store_memory`); `od-jit` и `od-isa` в S4-15 не менялись. Контракт обновлён в S4-15g (единый раздел). |
| Сделать: адаптировать проверенное memory-подмножество к RAM/MMU machine owner | S4-15a: load `48 8B 03` через tracked RAM (lib `s4_15a_paged_v2_qword_load_uses_tracked_ram_and_refuses_out_of_slice_inputs`); S4-15b: код берётся из guest RAM, а не из константы (`s4_15b_block_is_admitted_from_guest_ram_bytes_and_matches_the_interpreter` и ещё 5 `s4_15b_*`); S4-15d: store `48 89 03` (8 `s4_15d_*`); S4-15e: dword, disp8/disp32, RIP-relative (13 `s4_15e_*`). |
| Сделать: handles/leases | S4-15f: `reset`, освобождение MAP_JIT-региона при reset/Drop/инвалидации, отложенный release слота арены под borrow (5 интеграционных `s4_15f_*` + 3 lib `s4_15f_*`). |
| Сделать: precise exits | Guest `#PF` с точным `PageFaultRecord` (fetch и data, до D-бита и до записи) и отказы без effects; сверка с интерпретатором: `s4_15b_fetch_faults_precede_any_data_access_and_match_the_interpreter`, `s4_15c_fetch_fault_is_the_interpreters_page_fault_with_the_fetch_writes_kept`, `s4_15d_store_faults_precede_the_write_and_equal_the_interpreter`, `s4_15d_cpl3_store_to_a_supervisor_page_faults_before_the_write`, `s4_15e_fetch_faults_of_the_longer_forms_precede_any_data_access_and_equal_the_interpreter`. |
| Сделать: проверки fault/MMIO не обходятся direct host pointer; ownership не в runner | Операнд идёт только через scoped RAM-only `GuestMemory` (MMIO, ROM, framebuffer, cross-page, 2 MiB-лист отказываются до коммитящего walk): lib `s4_15a_paged_v2_qword_load_uses_tracked_ram_and_refuses_out_of_slice_inputs`, `s4_15c_mmio_operand_reaches_the_device_exactly_once_via_the_interpreter`, `s4_15d_mmio_store_is_refused_before_any_effect_and_not_executed`, `s4_15d_store_into_the_executing_code_page_is_refused_before_commit`, `s4_15d_store_with_an_untranslatable_operand_is_refused_without_any_data_walk_effect`. Ownership остался в `od-host`/`od-machine`, в runner не переносился. |
| Проверка: одни guest bytes через interpreter и JIT | Сравнение 16 GPR, RIP, masked RFLAGS, `PageFaultRecord`, упорядоченных writes и образа RAM: `s4_15b_block_is_admitted_from_guest_ram_bytes_and_matches_the_interpreter`, `s4_15c_supported_form_stays_on_the_jit_and_equals_the_interpreter`, `s4_15d_store_on_ram_equals_the_interpreter_and_sets_the_dirty_bit`, `s4_15e_the_existing_forms_through_execute_forms_equal_the_interpreter`; где формы в интерпретаторе нет — эквивалентная инструкция (`48 89 0B` с RCX=RAX; `48 8B 03` с RBX = эффективный адрес). |
| Проверка: fault до/после допустимых effects | `s4_15b_fetch_faults_precede_any_data_access_and_match_the_interpreter`, `s4_15b_code_accessed_bits_are_committed_without_preset_and_data_fault_matches` (fetch #PF, A-биты кода коммитятся), `s4_15c_fetch_fault_is_the_interpreters_page_fault_with_the_fetch_writes_kept`, `s4_15d_store_faults_precede_the_write_and_equal_the_interpreter` и `s4_15d_cpl3_store_to_a_supervisor_page_faults_before_the_write` (#PF до D и до записи), `s4_15e_effective_address_crossing_a_page_is_refused_before_the_data_walk`, `s4_15e_fetch_faults_of_the_longer_forms_precede_any_data_access_and_equal_the_interpreter`, `s4_15e_code_that_does_not_fit_the_page_is_refused_not_faulted`. |
| Проверка: MMIO side effects | `s4_15c_mmio_operand_reaches_the_device_exactly_once_via_the_interpreter` (ровно одно чтение устройства через интерпретатор, JIT устройство не вызывает), `s4_15c_unsupported_forms_are_interpreted_once_or_reported_unsupported` (MMIO store `48 89 0B`), `s4_15d_mmio_store_is_refused_before_any_effect_and_not_executed` (MMIO store `48 89 03` отказан до effects, не исполняется). |
| Проверка: live handle при reset/retirement | `s4_15f_reset_between_two_entries_readmits_and_equals_the_interpreter`, `s4_15f_the_block_stays_owned_after_its_execution_until_reset_or_drop`, `s4_15f_repeated_reset_and_late_drop_never_release_another_sessions_region`, lib `s4_15f_handle_dropped_under_a_borrow_is_deferred_not_leaked` и lib `s4_15f_handle_dropped_by_the_owner_during_a_native_call_is_freed_after_return` (handle арены роняется владельцем во время нативного вызова — слот освобождается после возврата). Retirement между vCPU — не проверен (S3-25, G7). |
| Проверка: реальное освобождение после допустимого срока | `s4_15f_free_slots_return_to_the_initial_count_after_reset_and_after_drop`, `s4_15f_generation_invalidation_replaces_the_block_without_leaking_the_region` (регион свободен после reset, Drop и инвалидации; `map_jit_region_live()`). Освобождение немедленное на safe point одного vCPU; grace period не реализован. |
| Готово: memory slice работает на production owner | Да, для форм из таблицы ниже. |
| Готово: unsupported JIT формы переходят в работающий interpreter | Да для `execute_or_interpret` в пределах allowlist `step_paged_mov` (`48 8B 03`, `48 89 0B`): все 7 `s4_15c_*` (UnsupportedForm, fetch/operand/publish-отказы, MMIO). Исключение — формы вне allowlist (см. вывод и ограничения). |
| Готово: в отчёте указано фактическое покрытие | Таблица форм ниже и список ограничений. |

### Покрытие форм (согласовано с JIT_CONTRACT.md)

| Форма | Байты | JIT (`execute_forms`) | JIT в `execute_or_interpret` | Интерпретатор `step_paged_mov` |
|---|---|---|---|---|
| `MOV RAX,[RBX]` | `48 8B 03` | да | да | да |
| `MOV [RBX],RAX` | `48 89 03` | да | да (без гарантии fallback) | нет в `step_paged_mov`: явный `Unsupported` |
| `MOV EAX,[RBX]` | `8B 03` | да | нет (`UnsupportedForm`) | нет (dword-профиль отдельной функцией, диспетчер его не вызывает) |
| `MOV RAX,[RBX+disp8]` | `48 8B 43 ib` | да | нет | нет |
| `MOV RAX,[RBX+disp32]` | `48 8B 83 id` | да | нет | нет |
| `MOV RAX,[RIP+disp32]` | `48 8B 05 id` | да | нет | нет |
| `67`-формы, store и dword со смещением | | `UnsupportedForm` | `UnsupportedForm` | нет |
| `MOV [RBX],RCX` | `48 89 0B` | `UnsupportedForm` | `UnsupportedForm`, затем интерпретатор | да |

Для форм, которые допускает только `execute_forms`, поведение в `execute_or_interpret` следует из того, что диспетчер вызывает `execute_with_store`; отдельного теста диспетчера на них нет.

### Вывод

**Готово выполнено с ограничениями:** memory slice работает на production owner (формы из таблицы), unsupported формы переходят в интерпретатор, кроме форм вне allowlist `step_paged_mov`: там явный `Unsupported` (это `48 89 03` после отказа store: MMIO/не-RAM, собственная code page, непереводимый операнд), без молчаливого успеха. Ограничение по `48 89 03`: Нет в одношаговом paged-профиле `step_paged_mov` (allowlist `is_paged_mov_instruction`: `48 8B 03` и `48 89 0B`); формы `48 89 03` и `8B 03` исполняются в od-isa (плоский s2-интерпретатор, dword-профиль `step_paged_mov_dword_read`, paged CPL0-набор на плате), но fallback к ним из диспетчера не подключён. Slice с гарантией fallback — `48 8B 03` (и `48 89 0B` исполняет интерпретатор); `48 89 03` — расширение JIT БЕЗ гарантии fallback при отказе (отказ явный, `Unsupported`, без молчаливого effect). Адресат остатка: S4-42 (fallback на paged CPL0-набор на Linux-трассе) либо отдельный подсрез od-isa/CPU_CONTRACT, если нужен одношаговый paged-профиль с `48 89 03` (вне «Где» S4-15). Тесты: 39 `s4_15{b,c,d,e,f}_*` (6 + 7 + 8 + 13 + 5) + `s4_17a` = 40 в `tests/system/stage4_memory_fault.rs`; lib `od-host`: `s4_15a_*` (1) + `s4_15f_*` (3) = 4.

### Ограничения (одним списком)

- Блок из одной инструкции; многоинструкционные блоки не сделаны.
- `67`-формы, store и dword со смещением вне allowlist `step_paged_mov`; JIT на них не расширялся; `execute_forms` не подключён к `execute_or_interpret`.
- `48 89 03`: Нет в одношаговом paged-профиле `step_paged_mov` (allowlist `is_paged_mov_instruction`: `48 8B 03` и `48 89 0B`); формы `48 89 03` и `8B 03` исполняются в od-isa (плоский s2-интерпретатор, dword-профиль `step_paged_mov_dword_read`, paged CPL0-набор на плате), но fallback к ним из диспетчера не подключён. Slice с гарантией fallback — `48 8B 03` (и `48 89 0B` исполняет интерпретатор); `48 89 03` — расширение JIT БЕЗ гарантии fallback при отказе (отказ явный, `Unsupported`, без молчаливого effect). Адресат остатка: S4-42 (fallback на paged CPL0-набор на Linux-трассе) либо отдельный подсрез od-isa/CPU_CONTRACT, если нужен одношаговый paged-профиль с `48 89 03` (вне «Где» S4-15).
- 2 MiB (и 1 GiB) страницы кода: tracked fetch идёт через 4 KiB walker и отказывает; код через границу 4 KiB-страницы и cross-page/2 MiB-операнд отказываются.
- MMIO/ROM/framebuffer операнд через JIT не исполняется (MMIO — только интерпретатором; отказанный `48 89 03` не исполняется ни одним движком диспетчера, см. ограничение по `48 89 03`).
- SMP и отложенное освобождение с grace period (S3-25, G7): освобождение немедленное, один vCPU; один MAP_JIT-регион на процесс (живой блок другой сессии даёт `Publish(MappingAlreadyLive)`).
- Нет интеграции в runner и Linux-трассу; `reset` вызывается только явно.
- Перформанс не измерялся (повторный tracked fetch, probe + walk на store).
- Store в активный блок и его инвалидация — S4-16i; aliases и кэш — S4-16.
- Ветки `JitHostFailure` тестом не покрыты (недостижимы из публичного API).

## S4-16f — контекст трансляции в ключе алиасов кода (реализовано; ревью Claude Opus, high: принято с ограничениями)

- **Ревью.** Ревью Claude Opus, high: принято с ограничениями. P3 закрыты: проверка CPL против CS-селектора state (`ContextRejected`), тексты.

- **Зачем.** `PagedCodeSession` (S4-16a–e) допускал блок под фиксированным контекстом фикстуры: `AddressSpaceId(1)`, `MappingVersion(1)`, `ExecutionContextId(1)`; CR3/CR0/CR4 сверялись на равенство снимку сессии, любая смена давала `PagingControlsMismatch` (или `UnsupportedAlias` для CR3) без обработки. Срез делает контекст частью ключа и заменяет отказ инвалидацией. Один vCPU, синхронно, неактивные блоки; поведение S4-16a–e при неизменном контексте не менялось.
- **Ключ.** `PagedCodeTranslationContext` (`crates/od-host/src/lib.rs`): корень CR3 (биты 35:12; PWT/PCD не влияют на трансляцию), CR0.WP, CR4.PSE, CR4.PGE, EFER.NXE, CPL (`access_context.privilege`). PCID/CR4.PCIDE `Long64ProbeConfig` не допускает, поэтому PCID в ключе нет; CR0.PG/PE, CR4.PAE, EFER.LME/LMA — обязательные константы снимка и различаться не могут. Биты вне ключа (CR0.TS/EM/MP/NE/AM, CR4.OSFXSR/OSXMMEXCPT, EFER.SCE) сброса не вызывают. Инъективный `id()` (24 бита страницы корня, 4 флага, 2 бита CPL, плюс 1) идёт в `CodeIdentity::address_space`. `ExecutionContextId` — монотонное поколение сессии (`context_generation`), растёт на каждую смену ключа: кэши диспетчера требуют строго возрастающий id (`GenerationNotAdvanced` при постоянном id отдельного контекста — выяснено на первом прогоне). Запись каждого alias хранит ключ допуска (`alias_contexts`); запись под другим ключом очищается до использования.
- **Правило сброса (консервативное; полный сброс сессии: обе кэш-записи, backing, диапазон, alias-привязка, счётчик `context_resets`).**
  - смена компонента ключа, наблюдаемая в CPU state при `execute_alias` / `step_guest_mov` / `execute_invlpg` / `step_guest_mov_wp` / `execute_mov_cr*` (замена прежнего `PagingControlsMismatch`), либо через `set_privilege` (CPL) / `set_efer` (NXE), либо retired `MOV CR0` с другим WP;
  - любой retired `MOV CR3`, включая то же значение (как S4-14d/h);
  - retired canonical `INVLPG` (прежнее правило S4-16e, теперь через общий `reset_session`);
  - доставленный `#PF`: явное уведомление `note_delivered_page_fault` (сессия сама `#PF` не доставляет; `#PF`, который лишь вернул шаг store, доставкой не считается и сессию не сбрасывает, как раньше).
  - Не сбрасывают: биты вне ключа, retired `MOV CR0` с тем же WP (в том числе то же значение), rejected/faulting `MOV CR3`/`MOV CR0`, set_efer/set_privilege без смены ключа.
  - `PagingControlsMismatch` остался только для отсутствующих CR0/CR3/CR4 в шаге интерпретатора, не-Long64 режима и невалидного снимка (например, PG сброшен); такой отказ сессию не трогает.
- **Новый код.** `od-machine` (`paged_memory_adapter.rs`, реэкспорт в `lib.rs`): `step_paged_mov_cr3_wp` (`MOV CR3,RAX`, WP=0/1) и `step_paged_mov_cr0_wp` (`MOV CR0,RAX`, переключение WP) над существующими профилями od-isa (`step_mov_cr3_wp`, `step_mov_cr0_wp`), через tracked fetch и тот же адаптер (`fetch_only_adapter`) с callback committed A/D. od-isa не менялся. `od-host`: `execute_mov_cr3`, `execute_mov_cr0`, `step_guest_mov_wp` (store при CPL0, WP=0/1 через `step_paged_cr0_wp_mov`; `step_guest_mov` по-прежнему WP=1), `set_privilege`, `set_efer`, `note_delivered_page_fault`, `translation_context()`, `context_resets()`. Любая ошибка принятия retired-эффекта (несоответствие old/new CR, невалидный снимок) сбрасывает сессию и возвращает `ContextEffectMismatch`/`ContextRejected`; сверка записей с code page выполняется и в этих путях.
- **Тесты** (`crates/od-host/src/lib.rs`, модуль `tests::s4_16f`, +7 (6 + тест CPL после ревью); в `stage4_memory_fault` не добавлялись, потому что тесты S4-16a–e живут в lib-тестах из-за доступа к полям сессии). Oracle — вторая идентичная RAM: `step_paged_mov` (`MOV RAX,[RBX]` читает immediate из alias), `step_paged_mov_cr3_wp`, `step_paged_cr0_wp_mov`.
  1. `mov_cr3_a_b_a_rekeys_and_never_reuses_old_block` (а) — корни A и B отображают один linear alias на разные code page; A → `MOV CR3`=B → снова A: результат JIT равен oracle (immediate A, B, A), `step`/state `MOV CR3` равны oracle, после каждого MOV CR3 resident = 0 и привязка снята, id/ключи A и B различны, возврат на A допускает заново (backing 0xa000, прежний ключ), дрейф CR3 без инструкции (хост-переключение) тоже даёт B.
  2. `cpl_change_gives_distinct_entries_and_user_supervisor_check` (б) — CPL0 допускает supervisor-only alias; `set_privilege(Ring3)` сбрасывает и меняет id; на CPL3 supervisor-страница даёт `Snapshot` fault, ничего не привязано, oracle-загрузка на CPL3 даёт `#PF`; user-alias допускается под ключом CPL3; возврат на CPL0 — заново.
  3. `cr0_wp_change_gives_distinct_entries_and_store_result` (в) — store в supervisor read-only страницу: WP=1 `#PF` (данные не изменены), после guest `MOV CR0` WP=0 сессия сброшена, store проходит; обе стороны совпадают с oracle (retired/fault и содержимое RAM); тот же `MOV CR0` и смена CR0.TS/CR4.OSFXSR не сбрасывают, возврат WP=1 сбрасывает и даёт прежний ключ.
  4. `routine_control_changes_are_not_paging_controls_mismatch` (г) — смена CR3 под сессией: `step_guest_mov` успешен, один сброс, ключ с новым корнем; смена TS/OSFXSR — без сброса; отсутствующий CR4 и сброшенный PG остаются `PagingControlsMismatch` без сброса.
  5. `mov_cr3_same_value_resets_session` (д) — `MOV CR3` тем же значением сбрасывает (resident 0, привязка снята, ключ тот же); rejected вход (`rflags_defined` вне маски) не сбрасывает.
  6. `efer_nxe_and_delivered_page_fault_reset_session` — `set_efer` с SCE не сбрасывает, невалидный EFER даёт `ContextRejected` без сброса, смена NXE сбрасывает и меняет id; `note_delivered_page_fault` сбрасывает.
  7. `cs_selector_cpl_mismatch_is_rejected_without_effect` (P3-3) — сессия на CPL0, state с CS-селектором `0x1b` (RPL 3) без `set_privilege`: `execute_alias` на supervisor-only странице даёт `ContextRejected(CplMismatch { cs_selector: 0x1b, key_cpl: 0 })`, state не изменён, resident 0, привязки нет, счётчик сбросов не вырос; `execute_mov_cr3` с тем же селектором — тот же отказ, state и сессия не тронуты; селектор `0x08` допускает supervisor-alias; после `set_privilege(Ring3)` и селектора `0x1b` supervisor-страница отвергается U/S-проверкой (`Snapshot`), не CPL-проверкой.
- **Мутации** (вручную, прогон `cargo test -p od-host --offline --lib s4_16f`, откат из копии, `cmp` после серии — идентично): ключ без CR3 (`cr3_root: 0`) → падают 1 и 4; ключ без CPL → 2; ключ без WP → 3; same-value `MOV CR3` без принудительного сброса → 5; `note_delivered_page_fault` без сброса → 6; `observe_controls` не сравнивает контроли → 1 и 4. Мутация правки P3-3 (проверка CS-CPL отключена, `cs_selector.filter(|_| false)`): падает тест 7 (`cs_selector_cpl_mismatch_is_rejected_without_effect`, 6 passed / 1 failed), после отката из копии `cmp` — идентично. Мутации без CPL и без WP ловятся утверждениями о сбросе/id/ключе, а не расхождением результата: `tracked_code_snapshot` повторяется на каждом `execute_alias` с актуальным `access_context`, поэтому устаревший блок не исполнится и без сброса; значение ключа здесь — идентичность записи и поколение для кэшей, а не единственная защита.
- **Результаты.** `cargo fmt --all -- --check` — расхождения только в `storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs` (уже в main), код 1; в изменённых файлах расхождений нет; `cargo check --workspace --all-targets --offline` — 0; `cargo test -p od-host --offline --test stage4_memory_fault` — 40 passed, код 0; `cargo test -p od-host --offline --lib` — 389 passed (382 + 7), код 0 (после правок P3; `stage4_memory_fault` — 40 passed, `cargo check --workspace --all-targets --offline` — 0, fmt — только прежние расхождения); `cargo test -p od-jit --offline --lib` — 455 passed, 2 ignored, код 0; `cargo test -p od-machine --offline --lib` — 1382 passed, 7 ignored, код 0.
- **Правки по ревью (P3).** (1) CPL: если в state есть CS-селектор, `(sel & 3)` обязан совпасть с CPL ключа, иначе `ContextRejected(ProbeConfigError::CplMismatch { cs_selector, key_cpl })` до любого эффекта (без сброса и без изменения state); проверка в `execute_alias` и в `observe_state_controls` (`step_guest_mov`, `execute_invlpg`, `step_guest_mov_wp`, `execute_mov_cr3`, `execute_mov_cr0`). Ограничение: state без CS-селектора (`None`) не проверяется, берётся CPL сессии, хост обязан вызвать `set_privilege`. Тест 7: `cs_selector_cpl_mismatch_is_rejected_without_effect`. (2) Документ `ContextRejected` уточнён: сессию не сбрасывает отказ до применения эффекта (`set_efer`, CPL-проверка, конфигурация до шага в `execute_mov_cr0`); сбрасывают только отказы уже retired записи CR в `execute_mov_cr3`/`execute_mov_cr0`. (3) `execute_mov_cr3`/`execute_mov_cr0` меняют state на месте: при `ContextEffectMismatch`/`ContextRejected` после retire state может содержать записанный CR и сдвинутый RIP; ветви недостижимы при профилях od-isa, при появлении пути вызывающий обязан считать state повреждённым (комментарии в коде, поведение не менялось). (4) В `JIT_CONTRACT` добавлено: доставку `#PF` сообщает хост (`note_delivered_page_fault`; без вызова сессия не сбрасывается, но обход по новым PTE даёт `UnsupportedAlias`/fault, а не устаревшее исполнение); смену CPL сообщает хост (`set_privilege`), расхождение с CS-селектором даёт `ContextRejected`; SMEP/SMAP/PKE/PKS/LA57/PCIDE/CD/NW не в ключе и не допускаются валидацией CR0/CR4, поэтому «не в ключе» безопасно.
- **НЕ сделано.** Активный блок (S4-16i), arbitrary writers (S4-16h), cross-page code (S4-16g), PCID/INVPCID/global pages, связь с `FetchTranslationCache` (правило сброса зеркалит `FetchCacheInvalidation::GuestOnly` из S4-14h, но общего кода нет), итоговый текст инвалидации в `JIT_CONTRACT` (добавлен только краткий подраздел S4-16f). Доставку `#PF` и смену CPL сессия не наблюдает сама: их сообщает хост-код. Интерпретатор store при CPL3 и WP=0 не добавлялся; `MOV CR4`, `WRMSR` (EFER) в интерпретаторе сессии не исполняются (EFER — только через `set_efer`).
- **Риски.** Полный сброс при любой смене ключа и любом `MOV CR3` консервативен и стоит повторных допусков (замеров нет, speed claim не делается). Сессия по-прежнему держит один backing и два alias; несколько контекстов одновременно не живут. Дрейф CR3/WP, замеченный в CPU state, молча принимается как новый контекст (правило «CPU state авторитетен»); вызывающий, перепутавший state, получит сброс вместо ошибки. `ExecutionContextId` теперь равен поколению сессии, а не константе 1: код, сравнивавший id с 1, в репозитории не найден (тесты lib проходят).

## S4-16g — код через границу 4 KiB-страницы (реализовано; ревью Claude Opus, high: принято с ограничениями)

- **Ревью.** Ревью Claude Opus, high: принято с ограничениями. P3 закрыты документами: `InvalidLength`-исключение для переполнения адреса, расхождение перечитывания с oracle в раскладке «таблицы в первой code page» записано как ограничение.

- **Зачем.** Блок `PagedCodeSession` (S4-16a–f) покрывал ровно одну физическую страницу кода: `tracked_code_snapshot_with_callback` отказывал по `length > 0x1000 - offset` до walk. Срез допускает блок, чьи байты лежат на двух guest-страницах, с двумя dependency spans и двумя backing. Один vCPU, синхронно, неактивные блоки; поведение S4-16a–f для блока в одной странице не менялось (тот же код пути, `second = None`).
- **Что сделано.**
  - `od-machine` (`crates/od-machine/src/bus.rs`, реэкспорт `TrackedCrossPageCodeSnapshot` в `lib.rs`): тело прежнего снимка вынесено в приватный `capture_code_page` (одна страница, один tracked fetch walk, чтение байтов, generation страницы); `tracked_code_snapshot_with_callback` вызывает его с прежней проверкой длины. Новый `tracked_code_snapshot_two_pages_with_callback`: диапазон `0 < length <= (0x1000 - offset) + 0x1000`, иначе `InvalidLength` до walk (длина вне `0 < length <= 0x2000 - offset`); исключение — переполнение адреса в последней странице адресного пространства (RIP в `0xffff_ffff_ffff_fxxx`, диапазон пересекает границу, `checked_add` переполняется): там отказ наступает после walk первой страницы, её A-биты закоммичены и переданы в callback, блок не привязан и не исполняется; зафиксировано как ограничение, интерпретатор делает тот же walk первой страницы; первая страница — walk как fetch по RIP; вторая (если диапазон её достигает) — отдельный fetch walk по адресу границы, но с `instruction_rip` = RIP инструкции, поэтому второй-страничный `#PF` имеет saved RIP = RIP и CR2 = граница, как у побайтового fetch интерпретатора. Страницы физически не обязаны быть соседними; у каждой свой `physical_base` и `backing_generation`. Отказ второго walk не откатывает эффекты первого (A-биты, сообщены через callback). Если второй walk записал A-бит в страницу таблиц, совпадающую с первой code page, первая страница перечитывается (байты и generation), чтобы оба куска описывали память на момент возврата из снимка (это не соответствие побайтовому oracle интерпретатора). Ограничение: для раскладки, где таблицы страниц лежат в первой code page, JIT может декодировать байты после установки A второго walk, а интерпретатор — до (он читает байты первой страницы до walk второй, и PTE второй страницы с битом A, попавший в байты блока, видит без A); такая раскладка не покрыта тестами (тест 7 держит PT[2] по 0x7010, блок 0x7ffd..0x7fff, вне байтов блока); вариант точного соответствия oracle — не перечитывать первую страницу и полагаться на последующую проверку версий (открытый вопрос).
  - `od-host` (`PagedCodeSession`): новое поле `second_backing_page` (сбрасывается `reset_session`, проверяется при повторном допуске вместе с `backing_page`/offset/length); `execute_alias` берёт двухстраничный снимок и строит **две dependency spans**: `[RIP, граница)` с `BackingId` первой физической страницы, её generation и `MappingId` первой линейной страницы, и `[граница, конец)` с `BackingId`/generation второй физической страницы и `MappingId` следующей линейной страницы (общие `address_space`, `mapping_version`). Для блока в одной странице остаётся прежняя единственная span. `reconcile_code_writes` проходит по обеим backing-страницам: committed write, пересекающий любую из них, сверяет generation этой страницы и инвалидирует обе кэш-записи (`BackingContentChanged`; для записи, сообщённой через пустой callback, инвалидация достигается несовпадением generation в ключе при следующем lookup, а не немедленным удалением записи); запись в другую страницу, в том числе внутри того же 16 KiB host-блока, не инвалидирует (generation на 4 KiB). Перед диспатчем кэш alias дополнительно сверяется с текущей версией обеих backing. Сам `execute_alias` повторяет снимок перед каждым lookup, поэтому резидентный блок не исполняется, если walk теперь падает.
  - Допуск ограничен тем, что уже есть: `NativeRegisterV1Admission::from_code` для всего диапазона (register-only; `MOV RAX,imm64` через границу). Диапазон длиннее двух страниц — `Snapshot(InvalidLength)` без walk (исключение для переполнения адреса в последней странице адресного пространства — см. выше: отказ после walk первой страницы); диапазон из двух страниц, не прошедший admission (нулевые байты — memory ADD), — `Admission(_)` без привязки сессии, публикации и изменения state.
  - Не менялись: `od-isa`, `paged_pump_backend`, `translation_probe_cache.rs`, `physical_memory.rs` (generation на каждую задетую 4 KiB-страницу уже был), S4-15a–f, S4-16a–f.
- **Тесты.** `crates/od-host/src/lib.rs`, `tests::s4_16g_*` (7; в `stage4_memory_fault` не добавлялись по той же причине, что S4-16a–f: нужны поля сессии) и `crates/od-machine/src/bus.rs` (+1). Раскладка: PML4/PDPT/PD/PT, linear 0x401000 → physical 0x7000 (первая), linear 0x402000 → physical 0x5000 (вторая, не соседняя), блок `MOV RAX,imm64` по смещению 0xffd (3 + 7 байт), два alias (0x401ffd, 0x601ffd), 16 KiB host-блок 0x4000–0x7fff содержит обе code page и страницу 0x6000. Oracle — вторая идентичная RAM и `step_paged_mov`: профиль интерпретатора возвращает `MOV RAX,imm64` как `Unsupported` только после полного побайтового fetch через обе страницы, поэтому он эталон для fetch-эффектов и `#PF`; значение RAX сверяется с контролем (тот же блок целиком в одной странице) и с ожидаемым immediate.
  1. `s4_16g_cross_page_block_is_admitted_with_two_backings_and_matches_oracle` (а) — блок допущен, retired_native/RAX/RIP равны контролю и ожиданию, `backing_page`=0x7000 и `second_backing_page`=0x5000, 5 A-записей walk (PML4, PDPT, PD, PT[1], PT[2]) дают RAM, побайтно равную RAM oracle; повторный вход через тот же alias не публикует заново, второй alias имеет свой слот.
  2. `s4_16g_write_to_first_page_invalidates_the_block` (б) — tracked `patch_alias_byte` в последний байт первой страницы: generation первой растёт, второй — нет, resident 0, повторный вход через оба alias публикует заново и исполняет новый immediate (старые байты не исполняются); физическая запись на первую страницу инвалидирует так же.
  3. `s4_16g_write_to_second_page_invalidates_the_block` (в) — физическая запись во вторую страницу (первая не тронута) и guest qword store через linear-alias второй страницы (`step_guest_mov`) инвалидируют; новый immediate исполняется.
  4. `s4_16g_write_to_unrelated_page_keeps_the_block_at_4k_granularity` (г) — записи в 0x6000 (внутри того же 16 KiB, между code page), 0x4800 (страница таблиц) и 0x8000 (другой 16 KiB) не инвалидируют (`code_invalidated == false`, resident 2, число публикаций не растёт, generation обеих code page прежние, generation записанной страницы выросла); запись внутри первой code page вне байтов блока инвалидирует (правило S4-16c, консервативная гранулярность страницы).
  5. `s4_16g_second_page_fetch_fault_is_exact_and_falls_back_to_the_interpreter` (д) — три причины: not-present (PFEC 0x14), NX (0x15), supervisor-only при CPL3 (0x15; бит I/D выставлен, так как EFER.NXE=1): `Snapshot(Access(PageFault))` с записью, равной `PageFaultRecord` oracle (saved RIP = 0x401ffd, CR2 = 0x402000), state (RIP/RAX/cr3) не изменён, ничего не скомпилировано и не привязано, RAM равна RAM oracle; fallback по S4-15c (один `step_paged_mov` на тех же state/bus после отказа JIT) даёт report/state/RAM, равные oracle. Отдельно: блок, резидентный до того, как PTE второй страницы стал not-present, при следующем входе не исполняется (RAX = 0, публикаций не прибавилось).
  6. `s4_16g_block_that_does_not_fit_two_pages_is_rejected_without_state_change` (е) — длины 0x1004, 0x2000, 0 дают `Snapshot(InvalidLength)` без walk (RAM не изменилась), state не изменён, ничего не привязано/опубликовано; 0x1003 (две полные страницы) отвергается `Admission`, сессия цела и допускает валидный блок после этого.
  7. `s4_16g_second_walk_ad_write_into_first_code_page_keeps_versions_consistent` — таблица страниц лежит в первой code page (PT = 0x7000): A-запись второго walk идёт после чтения байтов первой страницы; запись блока строится с обновлённым generation и переиспользуется при втором входе. Тест проверяет согласованность версий, а не соответствие побайтовому oracle: PT[2] лежит по 0x7010, вне байтов блока 0x7ffd..0x7fff, поэтому расхождение «до/после установки A» (см. ограничение выше) он не ловит.
  8. `od-machine::bus::tests::tracked_code_snapshot_two_pages_captures_each_page_with_its_own_generation` — раскладка байтов и физических баз, 5 A-записей, независимые generation страниц, диапазон в одной странице равен прежнему снимку, лимит 0x1003 / отказ 0x1004 и 0, прежний API сохраняет лимит одной страницы, `DescriptorTable`-origin отвергается, отказ второй страницы (CR2 = 0x6000, saved RIP = 0x5ffd).
- **Мутации** (вручную по одной, прогон `cargo test -p od-host --offline --lib s4_16g`, откат из копии, `cmp` после серии — идентично; M1–M7: падает ровно названный тест, остальные 6 проходят; M8/M9 — см. ниже):
  - M1 одна span на весь диапазон с первым backing (вторая зависимость убрана) → падает (в);
  - M2 `reconcile_code_writes` проверяет только первую страницу → (в);
  - M8 `reconcile_code_writes` проверяет только вторую страницу → падают (б) и (г) (запись внутри первой code page перестаёт инвалидировать);
  - M3 ошибка второго walk проглатывается, блок исполняется на нулевых байтах → (д);
  - M7 второй walk с `instruction_rip` = адресу границы → (д) (saved RIP не совпадает с oracle);
  - M4 гранулярность инвалидации 16 KiB вместо 4 KiB → (г);
  - M5 без перечитывания первой страницы после второго walk → тест 7;
  - M6 лимит три страницы вместо двух → (е);
  - M9 `second_backing_page` не привязывается → падают 6 из 7 (в том числе (а): `second_backing_page` не равен 0x5000; прочие — из-за неверной проверки привязки при втором alias и reconcile), проходит только (е).
- **Результаты.** `cargo fmt --all -- --check` — расхождения только в `od-host/src/storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs` (уже в main), код 1; в изменённых файлах расхождений нет; `cargo check --workspace --all-targets --offline` — 0; `cargo test -p od-host --offline --test stage4_memory_fault` — 40 passed, код 0; `cargo test -p od-host --offline --lib` — 396 passed (389 + 7), код 0; `cargo test -p od-machine --offline --lib` — 1383 passed, 7 ignored (1382 + 1), код 0; `cargo test -p od-jit --offline --lib` — 455 passed, 2 ignored, код 0.
- **НЕ сделано.** Активный блок (S4-16i); arbitrary writers (S4-16h); 2 MiB код-страницы; многоинструкционные блоки (допускается один register-only блок, исполняемый с бюджетом 1 инструкция в тестах); три и более страниц; патч второй страницы через `patch_alias_byte` (даёт `UnsupportedAlias`, патчится только первая; вторая — физической записью или guest store); production-обёртка «JIT или интерпретатор» для `PagedCodeSession` (fallback S4-15c в тесте (д) выполнен вызовом `step_paged_mov` на тех же state/bus, как правило S4-15c, но самой обёртки в `PagedCodeSession` нет); связь с `FetchTranslationCache`.
- **Риски.** Инвалидация идёт по generation физической страницы, а не по байтам блока: запись в любое место любой из двух зависимых страниц инвалидирует (консервативно, как S4-16c); для записи, сообщённой через пустой callback, инвалидация достигается несовпадением generation в ключе при следующем lookup, а не немедленным удалением записи. **Ограничения (P3).** (1) `InvalidLength` до walk (длина вне `0 < length <= 0x2000 - offset`); исключение — переполнение адреса в последней странице адресного пространства (RIP в `0xffff_ffff_ffff_fxxx`, диапазон пересекает границу, `checked_add` переполняется): там отказ наступает после walk первой страницы, её A-биты закоммичены и переданы в callback, блок не привязан и не исполняется; зафиксировано как ограничение, интерпретатор делает тот же walk первой страницы. (2) Для раскладки, где таблицы страниц лежат в первой code page, JIT может декодировать байты после установки A второго walk, а интерпретатор — до (он читает байты первой страницы до walk второй, и PTE второй страницы с битом A, попавший в байты блока, видит без A); такая раскладка не покрыта тестами (тест 7 держит PT[2] по 0x7010, блок 0x7ffd..0x7fff, вне байтов блока); вариант точного соответствия oracle — не перечитывать первую страницу и полагаться на последующую проверку версий (открытый вопрос). Сессия по-прежнему держит один набор backing (первая + вторая) на alias; alias с другой второй физической страницей даёт `UnsupportedAlias`. Oracle для успешного пути — не исполнение интерпретатором (профиль `step_paged_mov` не исполняет `MOV RAX,imm64`), а равенство RAM после fetch и значение RAX из контроля в одной странице. Граница на неканоническом втором адресе не тестировалась (второй walk вернёт ошибку доступа walker-а). Speed claim не делается.

## S4-16h — покрытие записывающих путей (writers) (реализовано; ревью Claude Opus, high: принято с ограничениями)

- **Ревью.** Ревью Claude Opus, high: принято с ограничениями. P3 закрыты текстом (M2, тесты с immediate, `deliver_paged_page_fault` — только чтением).

- **Зачем.** `PhysicalMemory::write_bytes` увеличивает generation каждой задетой 4 KiB-страницы при ЛЮБОЙ записи, но в `reconcile_code_writes` сессии доходят только committed ranges её собственных методов (`execute_alias`, `patch_alias_byte`, `write_physical_ram`, `step_guest_mov*`, `execute_invlpg`, `execute_mov_cr0/cr3`). Остальные писатели передают `|_| {}` или собственный callback. Срез доказывает тестом по каждому классу, что молчаливая запись не даёт исполнить устаревший блок. Код продукта не менялся (изменены только тесты `od-host/src/lib.rs`; helper `s4_16g_bus` теперь делегирует в `s4_16h_bus_with` с тем же содержимым).
- **Почему корректность не зависит от уведомления.** `execute_alias` перед каждым lookup заново делает tracked snapshot (байты и generation обеих страниц читаются из RAM), строит ключ из байтов и версий и только потом обращается к кэшу. Устаревший блок поэтому не находится: ключ другой. Явный цикл `invalidate(BackingContentChanged)` перед диспатчем и версия в `CodeIdentity` — два независимых слоя поверх байтов в ключе; мутация одного слоя (версии константой, либо цикл `invalidate` вырезан) тесты НЕ ловят, потому что корректность сохраняет другой слой. Это наблюдение, не дефект. Уведомление через `reconcile_code_writes` даёт немедленное освобождение записи кэша (`resident_entries() == 0`), а не корректность.
- **Таблица классов writers** (grep по коду; список по картe остатков, не исчерпывающий для всего репозитория):

| Класс | Место записи | Callback | Доходит ли до `PagedCodeSession` | Как инвалидируется блок | Тест |
|---|---|---|---|---|---|
| (1) NVMe DMA | `NvmeHandle::service(bus, committed_write)` -> `BusDma::write` (`od-machine/src/nvme/queues.rs`) -> `MachineBus::write_flat_bytes` -> `PhysicalMemory::write_bytes`; вызовы в `firmware_board.rs` (2244, 2380, 2610, 2639, 4399) и `tools/runner/src/s5_nvme_stand.rs:295` передают `\|_\| {}` | да, owner-callback получает диапазон каждой DMA-записи; в проде пустой | нет (сессия в проде этими вызовами не владеет шиной; при пересылке диапазонов в `reconcile_code_writes` — да) | generation растёт; следующий `execute_alias` берёт другой ключ и перекомпилирует | `s4_16h_nvme_dma_into_first_code_page_is_not_stale`, `..._into_second_code_page_...`, `..._across_both_pages_reports_both_ranges` (PRP1+PRP2, оба диапазона сообщены и через `reconcile_code_writes` дают `code_invalidated`) |
| (2) Linux loader | `linux_load.rs:429` `write()` -> `bus_mut().write_flat_bytes(.., \|_\| {})` (остальные `\|_\| {}` в файле: 977, 1011, 1153 — тесты); выполняется при загрузке, до старта гостя и до создания сессии | пустой | нет | generation растёт; сессия, созданная после загрузки, читает свежие байты | `s4_16h_loader_flat_writes_are_not_stale`, `s4_16h_loader_write_to_one_page_leaves_the_other_dependency_valid`. `write_flat_bytes` в od-machine `pub(crate)`, из od-host недоступен; для RAM он заканчивается в том же `PhysicalMemory::write_bytes`, что и публичный `patch_physical_ram_with_callback`, который тест и использует |
| (3) Host display/API | `od-host/src/display.rs` 381, 469 (`write_physical(.., \|_\| {})` в регистры MMIO и framebuffer), 1184 (тестовый `put`) | пустой; `write_bytes` для `Framebuffer` не вызывает callback и не ведёт generation | не требуется: RAM не пишется, framebuffer не RAM | generation RAM-страниц не меняются (проверены все 9 страниц), блоки остаются резидентными, повторный вход не публикует заново; fetch из framebuffer-alias даёт `Snapshot(_)` | `s4_16h_display_writes_never_touch_code_pages_and_cannot_become_code` |
| (4) Доставка прерывания/исключения | `od_machine::deliver_paged_external_interrupt` / `deliver_paged_page_fault` (`paged_exception_delivery.rs`): запись кадра на стек через `PagedExceptionMemory` -> `access_tracked_ram` с `committed_write`; в репозитории используется только в probe-модулях od-machine, в `od-host` нет; сессия знает только `note_delivered_page_fault` (полный сброс) | да (callback + `report.committed_writes`) | нет, если хост не перешлёт; `note_delivered_page_fault` для `#PF` сбрасывает сессию | generation обеих code-страниц растёт; ключ меняется | `s4_16h_exception_frame_written_over_code_is_not_stale` (кадр поверх обеих code-страниц: сохранённый RIP повторяет байты `48 b8 11`, сохранённый CS=8 становится новым immediate 0x811), `s4_16h_exception_frame_ranges_reconcile_when_forwarded`. Для `deliver_paged_page_fault` отдельного теста нет: запись кадра идёт через тот же `PagedExceptionMemory` с callback, что и протестированный `deliver_paged_external_interrupt`; покрытие проверено только чтением (`paged_exception_delivery.rs` около 589–611) |
| (5) JIT store | `PagedFetchedLoadSession::execute_with_store` -> `PagedMovStoreMemory::write` -> `access_tracked_ram(.., \|range\| writes.push(range))`; диапазоны уходят только в `PagedFetchedLoadReport::execution.committed_writes`, не в `PagedCodeSession` | собственный (в отчёт) | нет, и не может: один процессный MAP_JIT-регион, `PagedCodeSession::new` держит его весь срок жизни, поэтому блок `PagedFetchedLoadSession` и сессия alias-блоков взаимно исключаются (`MappingAlreadyLive` / `PagedFetchedLoadErrorKind::Publish`) | собственная сессия: ключ повторного использования включает physical page + generation + байты; сессия, созданная после store, читает свежие байты | `s4_16h_jit_store_into_a_fetched_block_page_is_not_stale`, `s4_16h_jit_store_and_a_code_session_never_hold_blocks_together` |

- **Тесты.** `crates/od-host/src/lib.rs`, `tests::s4_16h_*` (10). Общий `s4_16h_case`: прогрев двух alias cross-page блока S4-16g, запись writer-ом через `session.bus` (без уведомления сессии; либо с пересылкой диапазонов в `reconcile_code_writes` для режимов `reconcile = true`), затем вход через оба alias сравнивается с oracle (свежая шина с новыми байтами и отдельная сессия: `FallbackDispatchResult`, все GPR), ровно 2 новые публикации, повторный вход переиспользует. Для семи тестов на `s4_16h_case` каждый меняет исполняемый результат (другой immediate `MOV RAX,imm64`). Display-тест проверяет, что запись в framebuffer не трогает код; JIT-store-тест — отказ `UnsupportedForm` с нетронутым RAX; lease-тест — что регион занят одной сессией. Дополнительно проверяется, что writer продвинул generation нужной(ых) страницы (и не продвинул другую).
- **Мутации** (по одной, откат из копии, `cmp` идентично; после каждой правки `touch` файла, чтобы cargo не взял старую сборку):
  - M2 `write_bytes` не увеличивает generation (`od-machine/src/physical_memory.rs`): 13 passed / 17 failed (`--lib s4_16`); падают явные assert'ы generation (`after != before`) и режимы reconcile; проверки исполнения через oracle проходят — корректность в этой мутации держится на байтах в ключе. Это ожидаемо: два слоя версий (generation и явное уведомление) — защита в глубину;
  - M3 `reconcile_code_writes` всегда `Ok(false)` -> падают 2 (`..._across_both_pages_reports_both_ranges`, `..._ranges_reconcile_when_forwarded`);
  - M4 `execute_alias` собирает блок из устаревших байтов (`S4_16G_CODE` вместо снимка) -> падают 8 тестов с `PagedCodeSession` (display и fetched-store тест, которые не пользуются alias-кэшем, проходят);
  - M5 `PagedFetchedLoadSession`: ключ повторного использования без generation -> падает `s4_16h_jit_store_into_a_fetched_block_page_is_not_stale`;
  - M1/M1b (версии backing константой `1`; вырезан цикл `invalidate(BackingContentChanged)` перед диспатчем) -> 30 passed, НЕ падает: слои избыточны (см. выше). Мутация M2 вместе со снятым assert generation (M2b) выполнена: 2 passed / 8 failed (`--lib s4_16h`); все проверки исполнения проходят.
- **P1/открытые проблемы.** P1 не обнаружено: ни один класс не даёт исполнения устаревших байтов. Наблюдения: (а) в продакшен-коде нет мест, где `PagedCodeSession` и `nvme.service`/`deliver_*` работают с одной шиной — интеграция хоста, который пересылает диапазоны, не существует и не проверялась (тесты имитируют её через `session.bus`); (б) явный цикл `invalidate` в `execute_alias` избыточен по отношению к ключу — решение удалить его или оставить как защиту в глубину за координатором; (в) `write_flat_bytes` `pub(crate)`: реальный `load_linux_direct` из od-host не вызывался, проверен только терминальный `write_bytes`.
- **Результаты.** `cargo fmt --all -- --check` — расхождения только в `storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs` (уже в main), код 1; в изменённых файлах расхождений нет; `cargo check --workspace --all-targets --offline` — 0; `cargo test -p od-host --offline --test stage4_memory_fault` — 40 passed, код 0; `cargo test -p od-host --offline --lib` — 406 passed (396 + 10), код 0; `cargo test -p od-jit --offline --lib` — 455 passed, 2 ignored, код 0; `od-machine` не менялся (мутация M2 откатана, `cmp` идентично), его тесты не перезапускались.
- **НЕ сделано.** Активный блок (S4-16i); реальная проводка `reconcile_code_writes` в NVMe/loader/delivery; исчерпывающий обход всех `write_bytes` в репозитории (CPU-интерпретатор vCPU, APIC/PIC/PCI MMIO не RAM); ROM (`load_rom_bytes`) как код не проверялся.

## S4-16i — запись в байты активного блока (реализовано; ревью Claude Opus, high: принято с ограничениями)

- **Ревью.** Ревью Claude Opus, high: принято с ограничениями. P3 закрыты документами (`retired` — защита в глубину; формулировка освобождения).

- **Что изменилось относительно S4-15d.** S4-15d отказывал (`PagedStoreRefusal::ActiveCodePage`, до walk и записи) для store `48 89 03`, чей операнд лежит на физической странице, с которой выбран сам блок; диспетчер уходил в interpreter, который `48 89 03` не исполняет. Теперь отказ заменён разрешением: store проходит walk, проверки прав (Present/Write/User/WP), A- и D-биты и запись 8 байт по обычным правилам; при коммите команда завершается (RAX/RFLAGS/RIP как у интерпретатора), блок получает выход `MemoryOperationExecutionExitV2::CodeInvalidated` (вариант уже был в `od-host`, нового исхода в `od-jit` не добавлялось; `od-jit` не менялся). `#PF` по-прежнему до записи: `GuestFault`, байты и generation страницы не меняются, блок остаётся допущенным. Отказы `NonRamTarget` и `ProbeUnsupported` не менялись; store в другую code page не менялся (выход `Complete`, блок `Reused`).
- **Что изменено (файлы).**
  - `crates/od-machine/src/paged_memory_adapter.rs`: удалены вариант `PagedStoreRefusal::ActiveCodePage`, аргумент `active_code_page` у `with_paged_mov_store_memory`, поле адаптера и ветка отказа; документация обновлена. Адаптер больше не знает про «активную страницу».
  - `crates/od-host/src/lib.rs`: `PublishedCode::execute_paged_mov_rbx_rax_store_v2` после вызова блока меняет выход `Complete` на `CodeInvalidated`, если какой-либо committed write range пересекает 4 KiB-страницу `active_code_page` (диапазоны данных и, в патологическом случае, записи A/D в таблицу, лежащую на той же странице); `AdmittedFetchedLoad.retired` (новое поле), `PagedFetchedLoadSession::retire_after_code_invalidation`; ключ повторного допуска в `execute_form` и `execute_forms` теперь требует `!retired`; `execute_or_interpret` возвращает `CodeInvalidated` как финальный `PagedStepOutcome::Jit` (раньше любой исход, кроме `Complete`/`GuestFault`, был `JitHostFailure`); комментарии и документация полей обновлены.
  - `tests/system/stage4_memory_fault.rs`: тест `s4_15d_store_into_the_executing_code_page_is_refused_before_commit` заменён пятью `s4_16i_*` (ниже).
- **Граница наблюдения и выход.** Блок состоит из одной инструкции (store), поэтому её завершение (retire) и есть архитектурная точка наблюдения: RIP = `next_rip` (`CODE_RIP + 3`), не внутри переписанных байтов и не на следующей инструкции фикстуры (`48 89 0B` по смещению 3 не исполняется: образ RAM и RIP равны oracle, который делает один шаг). Старый блок после выхода не исполняется: он помечен `retired` и не используется повторно. Повторный допуск обеспечивает generation страницы: она растёт при любой committed записи в RAM (в том числе store тех же трёх байт), поэтому случай «совпавшей generation» после committed записи в активную страницу невозможен; поле `retired` — дополнительная защита в глубину. Следующий вход делает новый tracked fetch, снимок и ключ, блок публикуется заново (`Admitted`, `admissions` + 1) из новых байтов.
- **Время жизни allocation.** Блок заимствуется (`&PublishedCode`) на весь нативный вызов; его drop невозможен до возврата (`&mut self` у всех входов сессии, borrow checker). Вызов, который его пометил `retired`, его не освобождает: `jit_free_slots()` после выхода равен 0 (`map_jit_region_live()` истинно). Освобождение — при следующем входе, который дошёл до проверки ключа (старый блок снимается до публикации нового, слот заменён один в один, остаётся 0; освобождение происходит даже если дальше последуют `UnsupportedForm`, `Compile` или `Publish`; вход, отказавший раньше — контекст, CR3, operand range, fetch `#PF` — блок не освобождает), `reset()` (возврат к `ALL_FREE`) или drop сессии (механизм S4-15f; повторный `reset` — `false`). Наблюдать слоты изнутри нативного вызова из интеграционного теста нельзя (нет хука в store-адаптере): проверяется состояние сразу после возврата и после допуска/reset/drop; внутри вызова гарантия — borrow checker, не отдельный тест.
- **Тесты** (`tests/system/stage4_memory_fault.rs`; хелпер `active_page_store_vs_oracle` сравнивает полный `LongModeState`, `PageFaultRecord`, упорядоченные committed writes и образ RAM с interpreter `MOV [RBX],RCX` при RCX=RAX, диспетчер `execute_or_interpret`):
  1. `s4_16i_store_into_the_executing_code_page_commits_and_exits_like_the_interpreter` — (а): операнд рядом с байтами (offset 0x120), поверх байт (NOP-ы) и поверх с теми же тремя байтами: равенство oracle (RAX, RIP = +3, RAM, writes, retired = 1), выход `CodeInvalidated`, D-бит листа, рост generation, `Admitted`.
  2. `s4_16i_after_the_exit_the_old_bytes_never_run_and_new_bytes_are_admitted` — (б), (в): store переписывает блок в load `48 8B 03`; повторный вход — `Admitted`, fetched_bytes = load, RAX = marker (старый store-блок не перезаписал бы RAX и снова записал бы память), generation вырос, `admissions == 2`; store тех же байт — всё равно `Admitted`.
  3. `s4_16i_the_retired_block_stays_owned_until_the_next_admission_reset_or_drop` — (г): слоты `ALL_FREE` -> 0 после выхода -> 0 после следующего допуска -> `ALL_FREE` после `reset` (повторный `reset` — `false`) -> 0 -> `ALL_FREE` после drop.
  4. `s4_16i_store_into_another_code_page_does_not_exit_the_block` — (д): выход `Complete`, блок второго входа `Reused`, `admissions == 1`.
  5. `s4_16i_store_fault_on_the_executing_page_precedes_the_write_and_keeps_the_block` — (е): not-present и read-only лист, операнд рядом и поверх байт: `GuestFault` (равенство oracle по PageFaultRecord, A-битам, writes, RAM), RIP не сдвинут, страница и её generation не изменились, блок остаётся допущенным и при повторе даёт `Reused` и тот же `#PF`.
- **Мутации** (по одной, вручную, откат из копии, `cmp` идентично после каждой; `stage4_memory_fault`, 44 теста):
  - M1 не заменять выход на `CodeInvalidated` (оставить `Complete`) — падают 3: тесты 1, 2, 3 (`s4_16i_*`);
  - M2 ключ повторного допуска без `retired` и без generation (старый блок повторно входит) — падают 4: тест 2 (`s4_16i_after_the_exit_the_old_bytes_never_run...`), тест 3, а также `s4_15b_patched_code_byte_forces_readmission_and_never_runs_the_old_block` и `s4_15f_generation_invalidation_replaces_the_block_without_leaking_the_region`;
  - M3 освобождать блок сразу после возврата (`self.admitted = None` в `retire_after_code_invalidation`) — падает тест 3 (слот свободен до reset);
  - M4 выход `CodeInvalidated` при любой записи (условие пересечения заменено на `true`) — падают 4: тест 4 (`s4_16i_store_into_another_code_page...`) и `s4_15d_store_into_another_code_page...`, `s4_15d_store_on_ram...`, `s4_15d_load_only_entry...`;
  - M5 блок помечается `retired` при любом исходе (в том числе `#PF` и `Complete`) — падает 12 тестов, среди них тесты 4 и 5 (`#PF` не должен гасить блок).
- **Результаты.** `cargo fmt --all -- --check` — расхождения только в `tools/image-tool/tests/cli.rs` (и `od-host/src/storage_abi_v1.rs`, уже в main), в изменённых файлах нет; `cargo check --workspace --all-targets --offline` — 0; `cargo test -p od-host --offline --test stage4_memory_fault` — 44 passed (40 - 1 + 5); `cargo test -p od-host --offline --lib` — 406 passed; `cargo test -p od-machine --offline --lib` — 1383 passed, 7 ignored; `od-jit` не менялся (тесты не перезапускались).
- **НЕ сделано.** SMP и deferred retirement с grace period (S3-25): освобождение синхронное на одном vCPU; блоки из нескольких инструкций (store в середине блока, выход посреди блока и повторный вход по `PcMap`); 2 MiB code-страницы; активные блоки `PagedCodeSession` (alias-кэш, V3 `ActiveCodeBlockV3`); запись в активную страницу другим писателем во время исполнения блока (на одном vCPU синхронный вызов не допускает); производительность. Чекбокс S4-16 не ставится.
- **Риски.** (1) `CodeInvalidated` выдаётся по пересечению любого committed write с активной страницей, включая A/D-запись в таблицу, лежащую на странице кода (патологическая раскладка) — выход консервативен, а не по пересечению с байтами блока. (2) Единица инвалидации — страница (generation страницы), а не диапазон байт блока: store «рядом» с блоком тоже перепубликует его. (3) Внутриисполнительный контроль живости allocation опирается на borrow checker, не на тест. (4) Удаление параметра `active_code_page` — изменение публичного API `od-machine` (внешних вызывающих в репозитории не было). (5) `retired` — защита в глубину; одиночная мутация его отсутствия тестами не ловится (generation сама отсекает блок); отдельный тест «retired при неизменной generation» невозможен без изменения кода, т.к. committed запись в активную страницу всегда повышает generation.

## S4-16 — итог (закрытие, S4-16j; gate-ревью Claude Opus, high, 2026-11-16: вердикт (б) — принят с ограничениями)

Документальный срез: код не менялся. Контракт целиком — раздел «S4-16: code aliases and invalidation (closure, S4-16j)» в [JIT_CONTRACT.md](../specs/JIT_CONTRACT.md). Срезы S4-16a–i приняты по отдельности (ревью Claude Opus, «принято с ограничениями»); здесь они сведены к пунктам плана `docs/plans/STAGE_4.md`, S4-16. S4-16 принят с ограничениями (gate-ревью Claude Opus, high, 2026-11-16: вердикт (б)); чекбокс S4-16 в плане проставлен.

Тесты названы полными именами. Если путь не указан, тест лежит в `crates/od-host/src/lib.rs`, модуль `tests` (S4-16f — вложенный модуль `tests::s4_16f`); `stage4_memory_fault` — `tests/system/stage4_memory_fault.rs`. Имена сверены по `grep -n "fn s4_16"`.

### Пункты плана и чем они покрыты

| Пункт плана S4-16 | Чем покрыт (срез, тест) |
|---|---|
| Где: writers `od-machine`, code dependencies/cache/dispatch `od-jit`, system tests | Tracked-снимки и callbacks committed writes в `od-machine` (`MachineBus`, `step_paged_invlpg`, `step_paged_mov_cr3_wp`, `step_paged_mov_cr0_wp`), сессия `PagedCodeSession` и `PagedFetchedLoadSession` в `od-host`; `od-jit` в S4-16 не менялся; система — `stage4_memory_fault` (S4-16i). |
| Сделать: связать блоки с физическими code bytes | S4-16a: backing page + версия страницы `PhysicalMemory`, два alias одной physical page (`s4_16a_loader_patch_through_one_alias_invalidates_the_other_alias`); S4-16g: две dependency spans с отдельными backing и generation для кода через границу 4 KiB (`s4_16g_cross_page_block_is_admitted_with_two_backings_and_matches_oracle`). |
| Сделать: связать блоки с translation context | S4-16f: ключ `PagedCodeTranslationContext` (CR3 root, CR0.WP, CR4.PSE, CR4.PGE, EFER.NXE, CPL): `s4_16f::mov_cr3_a_b_a_rekeys_and_never_reuses_old_block`, `s4_16f::cpl_change_gives_distinct_entries_and_user_supervisor_check`, `s4_16f::cr0_wp_change_gives_distinct_entries_and_store_result`, `s4_16f::efer_nxe_and_delivered_page_fault_reset_session`, `s4_16f::mov_cr3_same_value_resets_session`, `s4_16f::routine_control_changes_are_not_paging_controls_mismatch`, `s4_16f::cs_selector_cpl_mismatch_is_rejected_without_effect`. |
| Сделать: уведомлять о committed write через любой разрешённый writer | S4-16c: физическая RAM-запись загрузчика (`s4_16c_physical_loader_write_invalidates_only_committed_code_page`); S4-16b: guest store (`s4_16b_guest_qword_store_invalidates_both_code_aliases`); S4-16d: A/D walk и patch (`s4_16d_tracked_page_walk_writes_reconcile_before_errors`); S4-16h: классы writers без уведомления сессии — NVMe DMA (`s4_16h_nvme_dma_into_first_code_page_is_not_stale`, `s4_16h_nvme_dma_into_second_code_page_is_not_stale`, `s4_16h_nvme_dma_across_both_pages_reports_both_ranges`), Linux loader (`s4_16h_loader_flat_writes_are_not_stale`, `s4_16h_loader_write_to_one_page_leaves_the_other_dependency_valid`), display (`s4_16h_display_writes_never_touch_code_pages_and_cannot_become_code`), кадр доставки (`s4_16h_exception_frame_written_over_code_is_not_stale`, `s4_16h_exception_frame_ranges_reconcile_when_forwarded`), JIT store (`s4_16h_jit_store_into_a_fetched_block_page_is_not_stale`, `s4_16h_jit_store_and_a_code_session_never_hold_blocks_together`). Оговорка: в проде эти writers не связаны с сессией (callback пустой или пишет в собственный отчёт); защита — ключ (байты + generation) при следующем lookup, а не немедленное удаление записи. |
| Сделать: обработать page-table remap | S4-16e: retired canonical `INVLPG` сбрасывает сессию, после remap блок допускается заново из нового backing (`s4_16e_retired_invlpg_resets_alias_session_after_remap`); S4-16f: retired `MOV CR3`, в том числе тем же значением (`s4_16f::mov_cr3_same_value_resets_session`). |
| Сделать: обработать изменение текущего active block | S4-16i: store `48 89 03` в физическую страницу выполняющегося блока коммитится, выход `CodeInvalidated`, блок `retired`: `s4_16i_store_into_the_executing_code_page_commits_and_exits_like_the_interpreter`, `s4_16i_after_the_exit_the_old_bytes_never_run_and_new_bytes_are_admitted` (оба в `stage4_memory_fault`). |
| Проверка: write alias → execute alias | S4-16a: `s4_16a_loader_patch_through_one_alias_invalidates_the_other_alias`; S4-16b: `s4_16b_guest_qword_store_invalidates_both_code_aliases`. |
| Проверка: одна guest page внутри host page (16 KiB) | S4-16c: запись соседней guest page по `0x6000` обновляет только её generation (`s4_16c_physical_loader_write_invalidates_only_committed_code_page`); S4-16g: `s4_16g_write_to_unrelated_page_keeps_the_block_at_4k_granularity` (0x6000 внутри того же 16 KiB, 0x4800, 0x8000 не инвалидируют; запись внутри code page вне байтов блока инвалидирует). |
| Проверка: loader patch | S4-16a: `s4_16a_loader_patch_through_one_alias_invalidates_the_other_alias`; S4-16c: `s4_16c_physical_loader_write_invalidates_only_committed_code_page`; S4-16h: `s4_16h_loader_flat_writes_are_not_stale`, `s4_16h_loader_write_to_one_page_leaves_the_other_dependency_valid`. |
| Проверка: cross-page code | S4-16g: `s4_16g_cross_page_block_is_admitted_with_two_backings_and_matches_oracle`, `s4_16g_write_to_first_page_invalidates_the_block`, `s4_16g_write_to_second_page_invalidates_the_block`, `s4_16g_second_page_fetch_fault_is_exact_and_falls_back_to_the_interpreter`, `s4_16g_block_that_does_not_fit_two_pages_is_rejected_without_state_change`, `s4_16g_second_walk_ad_write_into_first_code_page_keeps_versions_consistent`; `od-machine::bus::tests::tracked_code_snapshot_two_pages_captures_each_page_with_its_own_generation`. |
| Проверка: fail-before-commit | S4-16c: ROM write и диапазон через границу RAM/ROM не меняют bytes и generation (`s4_16c_physical_loader_write_invalidates_only_committed_code_page`); S4-16b/d: отказавший store после committed A/D-записи в code backing инвалидирует без operand commit (`s4_16b_fault_after_code_backing_ad_write_invalidates_without_operand_commit`, `s4_16d_tracked_page_walk_writes_reconcile_before_errors`); S4-16i (е): `#PF` store в активную страницу до записи, generation не меняется, блок остаётся (`s4_16i_store_fault_on_the_executing_page_precedes_the_write_and_keeps_the_block`). |
| Проверка: byte write в active block | S4-16i: `s4_16i_store_into_the_executing_code_page_commits_and_exits_like_the_interpreter` (операнд рядом с байтами, поверх байт и поверх теми же тремя байтами); `s4_16i_store_into_another_code_page_does_not_exit_the_block` (контроль). |
| Проверка: повторный lookup после invalidation | Во всех срезах: исполнение через alias после записи видит новые bytes и равно oracle (S4-16a, b, c, d, e, g, h); S4-16f — повторный допуск после возврата на прежний ключ; S4-16i — повторный вход `Admitted`, `admissions == 2` (`s4_16i_after_the_exit_the_old_bytes_never_run_and_new_bytes_are_admitted`). |
| Готово: старые bytes не исполняются после точки наблюдения | S4-16i: точка наблюдения — завершение (retire) store, RIP = `next_rip`, выход `CodeInvalidated` на границе инструкции, блок `retired`, повторный допуск обеспечивает generation (`s4_16i_after_the_exit_the_old_bytes_never_run_and_new_bytes_are_admitted`: старый store-блок не перезаписал бы RAX, а новый load записал marker); S4-16a–h — для неактивных блоков: ключ с байтами и generation (`s4_16h_*` — корректность держится на повторном snapshot, не на уведомлении). |
| Готово: удаление cache entry не освобождает используемую allocation | S4-16i (г): `s4_16i_the_retired_block_stays_owned_until_the_next_admission_reset_or_drop` (слоты 0 после выхода и после следующего допуска, `ALL_FREE` после `reset`/drop); S4-15f: `s4_15f_the_block_stays_owned_after_its_execution_until_reset_or_drop`, `s4_15f_free_slots_return_to_the_initial_count_after_reset_and_after_drop`. Во время нативного вызова блок заимствован (`&PublishedCode`), гарантия — borrow checker, не отдельный тест. |

### Ограничения (одним списком)

- Один vCPU, синхронно, без SMP; SMP и deferred retirement с grace period — S3-25.
- Writers (NVMe DMA, Linux loader, display, доставка прерывания, JIT store) в проде не связаны с `PagedCodeSession`: callback пустой или пишет в собственный отчёт; хост, пересылающий диапазоны в `reconcile_code_writes`, не существует и не проверялся (тесты имитируют его через `session.bus`). Корректность держится на повторном snapshot и ключе (байты + generation) при следующем lookup; уведомление даёт лишь немедленное освобождение записи. Обход writers не исчерпывающий.
- 2 MiB (и 1 GiB) код-страницы не поддержаны.
- Блоки из одной инструкции; многоинструкционные блоки (store в середине блока, выход посреди блока и повторный вход) не сделаны.
- Три и более страницы кода отвергаются (`InvalidLength` до walk); патч второй страницы через `patch_alias_byte` — `UnsupportedAlias`.
- Store и dword со смещением вне allowlist `step_paged_mov`; `48 89 03` — без гарантии fallback (S4-15).
- `retired` — защита в глубину: одиночная мутация его отсутствия тестами не ловится, generation отсекает блок сама; тест «retired при неизменной generation» невозможен без изменения кода.
- Перечитывание первой страницы после второго walk в раскладке «таблицы страниц в первой code page» расходится с побайтным oracle интерпретатора (S4-16g, открытый вопрос); раскладка тестами не покрыта.
- Неканонический адрес границы второй страницы: `#GP` не смоделирован (второй walk вернёт ошибку доступа walker-а), не тестировалось.
- Единица инвалидации — 4 KiB-страница (generation), а не диапазон байт блока; `CodeInvalidated` консервативен и по A/D-записи в таблицу, лежащую на странице кода.
- Доставку `#PF` и смену CPL сообщает хост (`note_delivered_page_fault`, `set_privilege`); PCID/INVPCID/global pages, SMEP/SMAP/PKE/PKS/LA57 не в ключе; связь с `FetchTranslationCache` не сделана.
- Активные блоки `PagedCodeSession` (alias-кэш) не поддержаны; production-обёртка «JIT или интерпретатор» для `PagedCodeSession` отсутствует.
- Производительность не измерялась (полный сброс при смене ключа и повторные snapshot консервативны); speed claim не делается.

## S4-09d/e — сводный контракт A/D и одно место коммита D (реализовано; ревью Claude Opus, high: принято с ограничениями)

Вердикт ревью: принято с ограничениями. P3 закрыты: поправлен счёт (3.5 — частично; 30/5/3 до S4-09f), оговорки grep-теста.

**S4-09e (код, без изменения поведения).** Раньше бит D листа коммитился в двух местах: в tracked access (через `table_writer`) и в транзакции LTR (прямой `PhysicalMemory::write_u64`, в обход seam). Теперь есть `commit_leaf_dirty` (`crates/od-machine/src/page_walk.rs`, перед `walk_long64_4k`): возвращает `Ok(None)`, если D уже стоит, `Ok(Some(AdUpdate))` после записи через `table_writer`, и `TrackedAccessError::TableWrite { bit: Dirty }` при отказе. Через него идут оба пути: `access_long64_4k_internal_windowed` и `commit_paged_ltr_descriptor_with_writer`. Публичная `commit_paged_ltr_descriptor` делегирует ей с `write_tracked_table_entry`; оба walk LTR теперь идут через `walk_long64_4k_with_writer` с тем же writer-ом (по умолчанию тот же байтовый эффект). Записи A остаются только в `walk_long64_4k_with_writer`; поиск по `AdBit::`, `1 << 5`/`1 << 6` и `write_u64` других мест записи A/D в production-коде не нашёл.

Тесты (`page_walk::tracked_access_tests`):
- `ltr_dirty_write_failure_commits_no_d_and_no_descriptor_image`: writer отказывает на записи D листа; LTR возвращает `TableWrite { Pt, 0x4020, Dirty }`, журнал попыток `[A x4, D]`, A остаются на всех уровнях, D нет, busy-образ дескриптора не записан, callbacks только четыре A. Мутация «писать D мимо seam» (прежний `write_u64`) не видит отказа, транзакция проходит: тест падает по ошибке и по busy-образу.
- `ltr_table_entry_writes_go_through_the_writer_seam`: все записи таблицы LTR (A walk и D) видны writer-у в порядке, затем один образ дескриптора; D при уже dirty листе не пишется.
- `leaf_dirty_is_committed_only_by_the_helper`: в production-части файла вне `commit_leaf_dirty` нет `AdBit::Dirty` и `| (1 << 6)`, и ровно один прямой `.write_u64(` (writer по умолчанию). Ограничение: grep-тест это структурный страж, не доказательство; он не ловит запись D в обход helper через `| 0x40` или новую константу без `AdBit::Dirty`; поведенческие тесты `ltr_dirty_write_failure_commits_no_d_and_no_descriptor_image` и `ltr_table_entry_writes_go_through_the_writer_seam` ловят обход seam в LTR; утверждение «других мест записи D нет в od-machine» держится на ручном поиске (найдены только два `write_u64` вне `page_walk.rs`: `bus.rs` (RAM-запись) и `translation_probe_cache.rs` (явный `write_pte`), оба не коммитят A/D).

**S4-09d (документ).** В `docs/specs/MACHINE_CONTRACT.md` добавлена таблица «путь × исход → A по уровням, D, операнд, callbacks» (раздел «Сводный контракт A/D страниц», якорь `ad-summary-contract-s4-09d`): 4 KiB и 2 MiB, межстраничные чтение/выборка и store (preflight A без D, затем побайтные записи с D), RMW, `LOCK`, MMIO served/unserved/ошибка backend, LTR, кадр доставки `#PF`, peeks и диагностика, hit кэша выборки. Решение «A на denied entry» записано как принятая семантика S4-09c; атомарность A/D (S6-25, `MEMORY_MODEL.md:28`) явно вне S4-09. В `CPU_CONTRACT.md` добавлена ссылка на таблицу без противоречий.

Покрытие: 38 ячеек, 30 покрыто тестом полностью, 5 частично (2.1, 3.5, 6.2, 6.3, 6.4: исход проверен, биты A/D не проверены или различимы лишь косвенно; для 3.5 тест не проверяет биты A/D первой страницы, то есть PTE `TREE_PT + 8`), 3 не покрыты тестом и покрыты чтением (4.1 RMW success, 4.2 RMW fault, 4.3 `LOCK`: тестов на биты таблицы нет, есть только на данные).

**Не сделано.** Атомарность A/D (compare-and-replace, повтор обхода) — S6-25. Страницы 1 GiB — S4-10. Тесты на биты таблицы для RMW и `LOCK` — ячейки 4.1-4.3 остаются открытыми. A на denied entry описано как принятая семантика, не как сверка с конкретным CPU.

**Проверки.** `cargo fmt --all -- --check`: код 1 только из-за известных расхождений `od-host/src/storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs`; в `page_walk.rs` отличий нет. `cargo check --workspace --all-targets --offline`: 0. `cargo test -p od-machine --offline --lib -- --test-threads=1 accessed dirty _ad_ tracked_access_tests`: 58 passed (55 прежних и три новых). `cargo test -p od-host --offline --test stage4_memory_fault`: 44 passed. `cargo test -p od-machine --offline --lib`: 1386 passed, 7 ignored (1383 и три новых).

## S4-09f — системная приёмка A/D через production-цикл набора (реализовано; ревью Claude Opus, high: принято с ограничениями)

Вердикт ревью: принято с ограничениями. Литералы A/D — SDM для битов, порядок и число записей — контракт S4-09d/зеркало; журнал без значений; настройка GDT с A=1 у дескрипторов — вне охвата, записано.

**Где живёт тест.** `run_cpl0_set_paged_2m_ram` (и вся цепочка `..._with_cpuid_boundary` -> `_with_policy` -> `_observed` -> `_with_cache`) помечен `#[cfg(test)]` и `pub(crate)` в `od-machine`, поэтому из `tests/system` (интеграционные тесты `od-host`) он недоступен. Сценарии лежат в `crates/od-machine/src/linux_direct/tests/stage4_ad_acceptance.rs` (подмодуль тестов `linux_direct`, рядом с остальными тестами цикла набора на плате Linux; доступ к ECAM даёт `LinuxDirectBoot`). Публичный API и поведение продукта не менялись; в `linux_direct.rs` добавлена одна строка `mod stage4_ad_acceptance;`.

**Сценарий (`a_d_acceptance_mixed_tree_sequence_through_the_set_loop_matches_the_literal_bits_and_write_order`).** Четырёхуровневое дерево из четырёх PML4-ветвей: корень 0 (4 KiB: код, стек, IDT/GDT, обработчик, окно на таблицу, read-only страница; 2 MiB-страница кода; поддерево с nonpresent PTE; PDE с R/W=0), далёкое дерево 4 KiB (данные, ECAM, пары для межстраничных записей), дерево 2 MiB (три страницы), дерево «первый доступ записью». У каждой entry A=D=0 (предустановки нет). Цикл шагается по одной инструкции: `stop` опрашивается в начале каждой итерации, первый опрос пускает инструкцию, второй завершает вызов (после одной retired-инструкции либо после одной доставки `#PF`). После шага сравниваются: (1) точный набор изменившихся qword в кадрах таблиц и их новые значения (литералы в тесте), (2) журнал `committed_write` в порядке (литералы `(физ. адрес, длина)`).

Инструкции (s0..s28): `NOP` (fetch), `MOV RAX,[RBX]` (read), `MOV [RBX],RCX` (write), `MOV [RBP],RCX` (первый доступ записью), `ADD [RDX],RCX` (RMW), `LOCK ADD [RSI],RCX`, `PUSH RCX`, `CALL +0`, `MOV [RDI],RCX` через границу 4 KiB, `MOV [RBX],RCX` в дерево «первый доступ записью», `MOV [RBX],ECX` в окно ECAM, `MOV [RBX],RCX` / `MOV RAX,[RBX]` / `ADD [RDX],RCX` на 2 MiB-страницах, `MOV RAX,[RBX]` на nonpresent PTE (`#PF`, доставка через IDT: s20), обработчик (`ADD RSP,8`, две загрузки, `MOV [RDX],RSI` через окно на таблицу, `IRETQ`: s21-s25), повтор загрузки (s26), `JMP rel32` на 2 MiB-страницу и `HLT` оттуда (s27-s28). Итог: 28 retired, 1 доставленный `#PF`.

Два дополнительных теста: `a_d_acceptance_write_protection_fault_after_earlier_accesses_keeps_a_on_every_level_and_no_d` (запись в страницу с R/W=0 на PTE, затем на PDE; CR0.WP=1, IDT не загружен, цикл останавливается на `Fault`) и `a_d_acceptance_cross_page_store_faulting_on_the_second_page_leaves_no_d_and_no_byte` (вторая страница nonpresent, затем read-only).

**Оракул.** Литералы A/D и порядок записей записаны в тесте руками из контракта (A=`0x20`, D=`0x40`, порядок «A уровней сверху вниз, D листа, операнд», для межстраничной записи preflight A обеих страниц, затем побайтные записи с D при первом байте каждой страницы); код под тестом их не вычисляет. Литералы A/D и порядка записей совпали с первого запуска (единственная правка после него касалась настройки теста: дескрипторы GDT для селекторов CS/SS, которые загружает `IRETQ`); расхождений с таблицей контракта не найдено. Второй оракул: тот же сценарий одним вызовом на идентичном дереве с предустановленными A (и D на листьях). Сверяются регистры, RIP, RFLAGS, RAM данных/стека/2 MiB-страниц и журнал data/stack-записей; единственные записи в кадры таблиц при предустановке: запись обработчика в PTE и A повтора. Это не независимый интерпретатор: `step_paged_mov` исполняет только qword `MOV` и не умеет `ADD`/`LOCK`/`PUSH`/`CALL`, ECAM и доставку `#PF`, поэтому прогон через него не заменяет сценарий. Различие (пре-A/D против нулевых) проверяет лишь, что A/D не влияют на вычисленный результат.

**Проверки (а)-(з).**

| Пункт | Результат | Где |
|---|---|---|
| (а) read: A на всех уровнях, D нет | покрыто | s1 (четыре уровня свежей PML4-ветви), s16 (2 MiB: A на PDE), s26 |
| (б) write: D на листе, A на всех уровнях | покрыто | s2 (D после чтения), s3 (A затем D на листе), s10 (свежая ветвь: A на четырёх уровнях, D на PTE, операнд), s14 (2 MiB: A на трёх уровнях, D на PDE) |
| (в) fetch: A на пути, D нет | покрыто | s0 (4 KiB), s28 (2 MiB), s21 (страница обработчика) |
| (г) nonpresent: A только у предшественников, D нет | покрыто | s20 (PDE поддерева получает A, сама nonpresent PTE не меняется); `Сист-3`, первая часть (вторая страница nonpresent). Nonpresent на 2 MiB-уровне системно не проверялся |
| (д) protection fault после прежних обращений: A на всех уровнях включая запрещающую entry, D нет | покрыто для R/W=0 при WP=1 на CPL0 | `Сист-2`: запрещающая PTE и запрещающая PDE (A стоит на PDE и на PTE ниже неё), операнд не записан. Supervisor-only страница из CPL3 не проверялась (см. ниже) |
| (е) межстраничная запись: A на обеих страницах, D на обеих при успехе; fault на второй странице: D нет | покрыто | s8 (A, A, D первой, 4 байта, D второй, 4 байта); `Сист-3`: вторая страница nonpresent либо read-only: A у первой страницы, у второй A предш. либо на всех уровнях, D нет, байтов нет |
| (ж) RMW: A и D, один D-callback | покрыто с уточнением | s4 (4 KiB), s18 (2 MiB). Запись A и запись D в entry — две отдельные записи (A при проходе чтения, D при проходе записи; журнал `[entry, entry, операнд]`), как в ячейке 4.1 контракта; «одной операцией» буквально нет, D-callback ровно один |
| (з) `LOCK`: то же | покрыто | s5 (`LOCK ADD m64, r64`: `[entry, entry, операнд]`) |
| прочее | покрыто | `PUSH`/`CALL` (s6: A, D стека; s7: только операнд), ECAM-store (s12: A, D, без записи операнда), доставка `#PF` и `IRETQ` (s20-s26) |

**Мутации** (механизм: копия `page_walk.rs` в scratchpad, правка, `cargo test -p od-machine --lib -- --test-threads=1 a_d_acceptance`, затем возврат из копии и `cmp` идентично). Все четыре роняют сценарий:

| Мутация | Результат |
|---|---|
| M1 `commit_leaf_dirty` никогда не пишет D | `Сист-1` падает на s2 (изменившихся entries нет; ожидалось D на PTE) |
| M2 A ставится на nonpresent entry | `Сист-1` падает на s20, `Сист-3` падает (вторая страница nonpresent: лишняя запись `0x5108022` в PTE) |
| M3 D ставится на read | `Сист-1` падает на s1 (PTE получает `0x5100063` вместо `0x5100023`) |
| M4 D ставится при отказе по правам (до возврата `Protection`) | `Сист-2` падает (`ro leaf`: PTE `0x500c061`), `Сист-3` падает (read-only вторая страница) |

После каждого прогона файл возвращался из копии, `cmp` показал идентичность, итоговый diff по `page_walk.rs` пуст.

**Не покрыто (и почему).**
- Supervisor-only страница из CPL3 (U/S): нужен вход в CPL3 (`with_user_mode`, TR, TSS.RSP0); системный A/D-тест для него не писался. Протокол «A на всех уровнях включая запрещающую entry, D нет» подтверждён для R/W=0 (Сист-2), а не для U/S; U/S остаётся за unit-матрицей S4-09c (`page_walk::tracked_user_supervisor_matrix_accumulates_us_at_every_level`).
- RMW и `LOCK` с fault (ячейка 4.2): в сценарии нет RMW на отказавшей странице.
- Системно не проверены: 2 MiB nonpresent/reserved/protection, fault выборки, fault на первой странице межстраничной записи, межстраничная запись через 2 MiB-лист, MMIO кроме ECAM-записи (остальные окна цикл набора не обслуживает по ячейке 5.4), отказ backend устройства (ячейка 5.3), `LTR` (ячейка 6), кэш выборки (ячейка 9), JIT.
- Атомарность A/D и повтор обхода (S6-25) вне S4-09. A/D дескрипторов GDT/IDT/TSS не проверялись (другой класс).
- Независимый интерпретатор-оракул не использован (см. выше).

**Проверки.** `cargo fmt --all -- --check`: код 1 только из-за известных расхождений `od-host/src/storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs`; в новом файле и в `linux_direct.rs` отличий нет. `cargo check --workspace --all-targets --offline`: 0. `cargo test -p od-host --offline --test stage4_memory_fault`: 44 passed (файл не менялся). `cargo test -p od-machine --offline --lib -- --test-threads=1 accessed dirty _ad_ tracked_access_tests`: 61 passed (58 прежних и три новых: имя модуля `stage4_ad_acceptance` содержит `_ad_`). `cargo test -p od-machine --offline --lib -- --test-threads=1 a_d_acceptance`: 3 passed.

## S4-09 — итог (закрытие)

Вердикт gate-ревью S4-09 (Claude Opus, high, 2026-11-16, по S4-09f): принять с ограничениями. Ограничения: U/S из CPL3 и 2 MiB nonpresent/protection системно не проверены; MMIO кроме ECAM и отказ backend — не системно; RMW/LOCK с fault (ячейка 4.2) — не покрыто тестом на биты таблицы; атомарность A/D (CAS) — S6-25; 1 GiB — S4-10; проверка A/D на реальном Linux (S4-09g) не делалась (опционально).
