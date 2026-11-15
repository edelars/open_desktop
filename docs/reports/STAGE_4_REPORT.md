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

**Что НЕ доказано.**
- Прерывания и таймер в CPL3: внешнее IRQ из CPL3 даёт `run_failed`; S4-39h
  пропущен. Сценарий `/init` проходит, потому что `/init` завершается после
  сна, а таймерные пробуждения шли в CPL0.
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

## Linux direct boot (S4-38/S4-39)

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
- IRQ из CPL3 не поддержано; S4-39h пропущен. Таймерная проверка гейтом не
  выполняется (в отчёте `halt_wakes` 2, но гейт его не требует).
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
- IRQ/timer в CPL3 (внешнее IRQ из CPL3 даёт `run_failed`, S4-39h пропущен);
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
- **Что сделано:** в `GuestOnly` запись в кадр таблицы ничего не сбрасывает, а только считается. Гостевые сбросы всего кэша: после успешно выполненного `INVLPG m` (подглядывание перед шагом по образцу `iretq_at_rip`, только чтение: префиксы `64`/`65` и REX без W, затем `0F 01`, ModRM reg=7, mod≠3; при нулевом retire кэш сохраняется), после `MOV CR3` (как раньше, в обеих политиках; тем же значением тоже), при любом изменении `state.cr0`/`state.cr4` за шаг (CR4.PGE) и при каждом `#PF`, доставленном гостю через IDT (правка по ревью: `#PF` инвалидирует трансляции своего адреса, SDM Vol. 3A §4.10.4.1, ссылка по памяти, с первоисточником не сверена; сбрасывается весь кэш, что допустимо, обработчик вправе менять PTE без `INVLPG`; счётчик `fault_flushes`). Кэш сохраняется только при отклонении, при ошибке без доставки исключения и при доставке `#GP(0)`. Подглядывание и сравнение CR выполняются только в `GuestOnly`: `WriteWatch` не менялся ни по поведению, ни по счётчикам. Кэш по-прежнему создаётся заново в каждом вызове.
- **Исправление контракта:** прежний текст CPU_CONTRACT утверждал, что `INVLPG` и CR0/CR4/EFER сбрасывают кэш S4-13. Это не так: `INVLPG` в наборе CPL0 — пустая операция (S4-38L7ab), `MOV CR0/CR4` не дают `TranslationContextChange`. Текст исправлен; сброс по ним есть только в `GuestOnly`.
- **Тесты (новые):** `guest_only_invlpg_after_a_pte_store_runs_the_new_code`; `guest_only_without_invlpg_keeps_the_stale_translation_and_write_watch_does_not` (оба исхода: `GuestOnly` видит старый код A, `WriteWatch` новый B); `write_watch_ignores_the_invlpg_of_the_guest`; `guest_only_reloading_cr3_with_the_same_value_runs_the_new_code`; `guest_only_toggling_cr4_pge_runs_the_new_code` (PGE 1, 0, 1); `guest_only_rewriting_cr0_runs_the_new_code`; `guest_only_a_cr4_write_of_the_same_value_flushes_nothing`; `guest_only_two_mib_leaf_made_nx_faults_exactly_after_invlpg` (PFEC 0x11) и `guest_only_two_mib_leaf_made_not_present_faults_exactly_after_invlpg` (PFEC 0x10) — без `INVLPG` `GuestOnly` допускает продолжение по кэшу, `WriteWatch` даёт fault сразу после записи PDE; `the_bool_entry_point_is_the_write_watch_cache`; юнит-тесты `fetch_translation_cache::tests::guest_only_counts_a_table_write_and_keeps_every_entry_until_a_guest_flush` и `write_watch_is_the_default_policy_and_ignores_nothing`. После ревью добавлены: `guest_only_a_delivered_page_fault_drops_the_cached_translation` (код на X закеширован, `store` в неотображённую страницу даёт `#PF`, обработчик через IDT меняет PTE страницы X без `INVLPG` и возвращается; после рестарта исполняется код F2, а не F1; для `WriteWatch` результат тот же, `fault_flushes` = 0); `guest_only_prefixed_invlpg_forms_run_the_new_code` (`65 0F 01 38`, `41 0F 01 3F`, `64 41 0F 01 3F` сбрасывают кэш); `the_invlpg_peek_recognizes_exactly_the_decoded_shapes` (таблица из 26 форм: `SWAPGS` `0F 01 F8`, `RDTSCP` `0F 01 F9`, `0F 01 C1`, `0F 01 FB` и другие `mod = 3`, `LGDT`/`LIDT`/`SGDT`/`SMSW`, REX.W, `66`/`F0`/`26`, REX не перед опкодом — не `INVLPG`).
- **Мутации (каждая откатана, файлы совпадают с копиями до мутации):** отключение сброса по `INVLPG` ломает 3 теста (remap и два теста 2 MiB-листа); отключение сброса по `MOV CR3` ломает `guest_only_reloading_cr3_with_the_same_value_runs_the_new_code` и `cache_mov_cr3_drops_the_translations_of_the_old_tree`; сравнение только CR0 ломает тест PGE; сравнение только CR4 ломает тест CR0; сброс по записи PTE в `GuestOnly` ломает 5 тестов, в том числе юнит-тест и тест без `INVLPG`. После ревью: отключение сброса по доставленному `#PF` ломает `guest_only_a_delivered_page_fault_drops_the_cached_translation`; удаление пропуска префикса `64`/`65` или REX ломает `guest_only_prefixed_invlpg_forms_run_the_new_code` и таблицу форм; удаление проверки `mod != 3` ломает таблицу форм. Все мутации откатаны, файлы сверены с копиями побайтно.
- **Не закрыто:** кэш не постоянный между вызовами (S4-14i); нет кэша данных, постраничного `INVLPG`, PCID, SMP; `GuestOnly` ни в раннер, ни в Linux-плату не подключён; ревью не пройдено; полный S4-14 открыт (в том числе поведение без инвалидации проверено лишь в том, что оно разрешено, а не как гарантия).
