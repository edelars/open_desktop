# Известные недоделки и проблемы

Реестр проблем, обнаруженных в ходе работы и не устранённых к завершению задачи. Правила ведения — в [AGENTS.md](../AGENTS.md#документирование-результатов-и-открытых-проблем). Планируемые возможности сами по себе не являются найденными проблемами и описываются в [ROADMAP.md](ROADMAP.md) и [планах этапов](plans/).

Для каждой записи указывать:

- Идентификатор, дату обнаружения и затронутый компонент.
- Суть недоделки или проблемы и её последствия.
- Условия воспроизведения или основание обнаружения, ссылки на исходники и результаты проверок при наличии.
- Статус: открыта, в работе или решена; необходимый следующий шаг.
- При решении — результат проверки и ссылку на обновлённую документацию реализации.

## Записи

<a id="issue-109-s4-31-dsdt-missing-cpu-device"></a>

### ISSUE-109 — S4-31 DSDT не объявляет processor device для Local APIC

- **Дата обнаружения:** 2026-11-16, при независимом ревью S4-31f.
- **Компонент:** `firmware/uefi/src/acpi/dsdt.rs`, `firmware/uefi/src/acpi/madt.rs`, S4-31.
- **Статус:** решена в DSDT follow-up S4-31b от 2026-11-16; независимое ревью пройдено.
- **Основание и последствия:** при обнаружении MADT объявлял один Processor Local APIC record с UID 0, а DSDT не содержала matching processor device object. ACPI 6.4 §5.2.12.2 требует для каждого процессора обе записи; с 6.3 processor device object задаётся как `Device` с `_HID = ACPI0007` и целочисленным `_UID` ([официальная спецификация](https://uefi.org/htmlspecs/ACPI_Spec_6_4_html/05_ACPI_Software_Programming_Model/ACPI_Software_Programming_Model.html)). Это пробел соответствия и CPU enumeration; отказ загрузки Linux этим не доказан.
- **Реализация:** DSDT теперь всегда выводит `\_SB.CPU0` с `_HID` строкой `ACPI0007` и `_UID` 0 перед manifest-устройствами. Golden и const-тесты обновлены; тест сравнивает CPU0 AML с единственной записью MADT и её UID 0. Описание layout обновлено в [ACPI_SMBIOS_TABLES.md](specs/ACPI_SMBIOS_TABLES.md).
- **Проверки:** firmware tests — 155 passed; `cargo fmt --check`, `cargo check --offline --target x86_64-unknown-none`, release build `od-acpi-blob`, package fixture `--compare` и machine-side `stage4_blob_is_built_for_this_boards_manifest` прошли. ACPICA `iasl` 20260408 разобрал построенную DSDT длиной 258 байт и полный fixture DSDT длиной 273 байта; обе содержат `CPU0`, строковый `_HID` и integer `_UID`. Это не проверка загрузки ОС; независимый reviewer подтвердил fixture/checksums и 155 firmware tests. Fixture ACPI blob: 1104 bytes, SHA-256 `6fc25b091348d8ad314bdd0f4f579ec9dd3969c961020a7a61d2daeb8dc26d4d`.
- **Результат:** issue исправлена. Fixture ACPI blob — 1104 bytes, SHA-256 `6fc25b091348d8ad314bdd0f4f579ec9dd3969c961020a7a61d2daeb8dc26d4d`. См. [S4-31b CPU namespace follow-up](reports/STAGE_4_S4-31B_2026-11-08.md) и [fixture README](../tests/fixtures/guest/stage4/acpi_blob/README.md). S4-31 parent acceptance и S5-08 остаются отдельными gates.

<a id="issue-100-stage4-uart-echo-cli-test-exit-code-mismatch"></a>
### ISSUE-108 — слияние линий S4 и S5: не покрыты тестами совместные режимы наблюдений

- **Дата обнаружения:** 2026-11-15, ревью слияния `63027b0` (codex, линия этапа 5, в main).
- **Компонент:** `crates/od-machine/src/paged_memory_adapter.rs` (`run_cpl0_set_paged_2m_ram_observed`), `crates/od-machine/src/linux_direct.rs`, `tools/runner/src/linux_direct.rs`.
- **Статус:** открыта; поддержка и тесты — отдельным срезом.
- **Суть и последствия:** (1) Пара `--execution-observations-out` + `--checkpoint-table` не поддержана: публичные функции наблюдений S5 не принимают таблицу checkpoints S4, поэтому `parse` отвергает пару явным отказом (тест `execution_observations_with_checkpoint_table_is_refused`). Цикл платы (`run_cpl0_set_paged_2m_ram_observed`) принимает оба наблюдателя сразу, но совместный запуск checkpoints + observations тестами не проверялся. (2) Совместное включение `FetchCacheInvalidation::GuestOnly` (S4-14h) и наблюдений S5 тестами не проверялось. (3) Захват post-IRET внутри цикла платы не покрыт тестом через user-mode фасад: сценарий «`#PF` в CPL3 → ремонт обработчиком → `IRETQ` → снимок» и `Some(0)` (индекс выбранного доставленного `#PF`) проверены только на уровне адаптера (`post_iret_snapshot_*`), не на уровне `run_linux_direct_board_set_user_mode_with_post_iret_index`.
- **Статус user mode под APIC (S4-38L9-um, 2026-11-15):** пункт user mode под `stage4a-apic` закрыт кодом и синтетикой (отказ `--user-mode` снят, см. ISSUE-106); пары наблюдений под этим профилем (`--execution-observations-out` с `stage4a-apic`) отдельными тестами не покрыты; живой прогон `stage4a-apic --user-mode` без пары наблюдений выполнен в S4-38L9J (диагностический, `exit_kind: backend` после паники, см. ISSUE-106, пункт 5), живой прогон с `--execution-observations-out` под APIC не выполнялся: открыто.
- **Условия обнаружения:** чтение результата слияния; разрешённые конфликты `paged_memory_adapter.rs`, `firmware_board.rs`, `linux_direct.rs`.
- **Следующий шаг:** срез с тестами (a) user-mode фасад с `#PF` → ремонт → `IRETQ` и `Some(0)`; (b) checkpoints + observations в одном прогоне цикла, затем снятие отказа в `parse`; (c) `GuestOnly` + observations.

<a id="issue-107-stage4-uart-echo-cli-test-exit-code-mismatch"></a>

### ISSUE-107 (закрыто) — S4 UART echo CLI test: exit-code assertion mismatch

- **Дата обнаружения:** 2026-11-14 (линия этапа 5, в реестре codex значилась как ISSUE-100; переименована при слиянии с main, где номер 100 занят).
- **Компонент:** `tools/runner/tests/stage4_probe_cli.rs` (`uart_echo_profile_checks_rx_expectation_budgets_and_stop`) и соответствие кодов выхода пробы UART echo Stage 4.
- **Статус:** закрыта. Исправление: коммит `6265daa` («S4-44b probe_cli exit code»), он в main; после слияния линии этапа 5 (`63027b0`) тест `stage4_probe_cli::uart_echo_profile_checks_rx_expectation_budgets_and_stop` проходит.
- **Суть и последствия (исторически):** полный прогон `od-runner` останавливался на проверке кода выхода `6` для `--stage4-uart-echo-probe --expect-byte 1`, а runner возвращал `5`; последующие JSON-проверки теста не выполнялись.
- **Следующий шаг:** нет.

### ISSUE-106 — S4-38L8d: стоп на записи RTE пина 9 (SCI) и непроверенные части APIC-профиля

- **Дата обнаружения:** 2026-11-15, живой прогон S4-38L8d (`docs/reports/data/stage4/s4-38l8d-apic-check-timer.json`).
- **Компонент:** `crates/od-machine/src/io_apic.rs` (`valid_low`), `pit_ioapic_irq_source.rs`, `firmware_board.rs`, профиль `X8664V1Stage4AApic`.
- **Владелец:** следующие срезы S4-38 (UART, MSR 0x1B, L9, L10); S4-38L8e и S4-38L8f (`wake_ns`) выполнены.
- **Статус:** частично закрыта: срезы L8e (запись RTE SCI на pin 9) и L8f (`wake_ns` источника IOAPIC) реализованы, остался пункт (4) (диагностически подтверждён живым прогоном S4-38L9J, не закрыт; открыт пункт (5): запись LVT Timer после паники) (пункт (3), UART → INTIN4, закрыт кодом и тестами в S4-38L8h; пробуждение HLT по таймеру LAPIC закрыто кодом и синтетикой в S4-38L8i, живой прогон открыт; ожидает независимого ревью и принятия L8e, L8f, L8h, L8i).
- **Статус L8f (2026-11-15):** пункт (2), часть `wake_ns`, закрыт кодом и синтетическими тестами (ожидает независимого ревью и принятия): `PitIoapicIrqSource::wake_ns` даёт срок следующего ребра PIT, если entry 2 не замаскирован, LAPIC включён и вектор не блокируется TPR/ISR; замаскированный entry и заблокированный вектор не будят (HLT остаётся `halted`). Живой прогон `stage4a-apic` для L8f не выполнялся. Остаются: пробуждение HLT по таймеру LAPIC (срок за владельцем часов, в живом прогоне не проверено), MSR 0x1B (ISSUE-105), UART → INTIN4, L9, L10, user mode под APIC-профилем.
- **Статус L8e (2026-11-15):** IOAPIC принимает RTE pin 9 как level/active-low без маски (`0xa020`) под профилем `stage4a-apic`, уровневая семантика (Remote IRR, EOI, повтор при поднятой линии) обобщена на pin 9; прочие неподдержанные поля отвергаются как прежде. Живой прогон (`docs/reports/data/stage4/s4-38l8e-sci-pin9.json`, диагностический, не засчитывается): pin 9 пройден, достигнут C16 `kernel_execve` (retired 2 321 216 080), стоп `user_entry` (retired 2 321 470 691) (iretq в CPL3 без `--user-mode`; C17 не достигнут). Резерв вектора уровневой формы pin 9 действует и под маской: маскированная SCI-форма с вектором V отвергается, если V уже занят (разблокированный entry 2, уровневый INTIN4, LVT Timer того же LAPIC), и наоборот: пока pin 9 в SCI-форме держит V, уровневый INTIN4, запись entry 2 и LVT Timer с V отвергаются, а фронт INTIN4 с V не доставляется (`edge` отказывает). Принятые пути не затронуты: UART probe пишет только INTIN4 = 0x51, pin 9 остаётся MASK; окна IOAPIC под stage4a закрыты. Для среза источника SCI: если линия поднята до настройки ОС (`sci_asserted`), переход записи pin 9 в SCI-форму или смена её вектора отвергается (`UnsupportedConfiguration`, как у INTIN4): ОС должна настроить запись до подъёма линии, либо источник поднимает её позже. Источника SCI в плате нет, доставка SCI гостю не проверялась.
- **Статус таймера LAPIC (S4-38L8i, 2026-11-15):** пункт про пробуждение HLT по таймеру LAPIC закрыт кодом и синтетическими тестами (ожидает независимого ревью и принятия): ветка простоя цикла платы берёт минимум срока источника и срока таймера LAPIC BSP (`idle_wake_ns`, `lapic_timer_wake_ns`; нужны владелец часов и источник IOAPIC), `idle_to` под источником IOAPIC принимает вектор LAPIC, будящий ЦП; вектор идёт через LAPIC IRR и IDT, EOI гостя. One-shot и periodic будят `HLT` ровно на сроке; замаскированный LVT не будит; stage4a и 8259 не менялись. Живой прогон `stage4a-apic` с пробуждением по таймеру LAPIC не выполнялся: открыто.
- **Статус UART → INTIN4 (S4-38L8h, 2026-11-15):** пункт (3) закрыт кодом и синтетическими тестами (ожидает независимого ревью и принятия): `UartIoapicIrqSource` передаёт выход прерывания UART как уровень INTIN4 под профилем `stage4a-apic`, вектор идёт через LAPIC IRR и IDT гостя, `HLT` просыпается по ожидающему байту RX; stage4a (8259 IR4, S4-39i) не менялся. Живой прогон UART под `stage4a-apic` не выполнялся; открыто: Прогон, который начинается при остановленном (`Halted`) ЦП, а источник уже имеет доставляемый вектор (например, байт RX подан между двумя вызовами цикла), завершается `run_failed` («an external interrupt of the board source was refused by the CPU delivery», retired 0); байт остаётся в RBR. Это свойство цикла платы для любого источника (PIT, 8259, UART), а не только UART; исправление — вход в цикл платы при `Halted` (общий блок с веткой после кванта) — открыто, до будущего среза с живым вводом RX или прогоном кусками. Не блокер для живого прогона: фасад вызывает плату один раз, RX во время прогона не подаётся. Пробуждение по таймеру LAPIC в живом прогоне не проверено.
- **Статус user mode под APIC (S4-38L9-um, 2026-11-15):** пункт (4) закрыт кодом и синтетическими тестами (ожидает независимого ревью и принятия): отказ `--user-mode` для `stage4a-apic` снят в `od-machine` и в разборе раннера; IRQ из CPL3 (таймер LAPIC и PIT -> IOAPIC INTIN2) доставляется через LAPIC IRR и IDT по `TSS.RSP0` (ISSUE-090), `IRETQ` возвращает в CPL3, при IF = 0 вектор ждёт (`irq_refused_masked`), результат `diagnostic_not_counted`; stage4a не менялся. Живой прогон `stage4a-apic --user-mode` не выполнялся: открыто. Наблюдение: таймер LAPIC, армированный гостем внутри кванта, виден на следующей границе кванта (запись в LAPIC не заканчивает квант); в синтетике его армирует хост.
- **Статус discovery на APIC (S4-38L9J, 2026-11-15):** живой прогон `stage4a-apic --user-mode` выполнен (два холодных процесса, отчёты побайтно равны; `docs/reports/data/stage4/s4-38l9j-apic-discovery-run.json`), вердикт `linux-direct-discovery` `PASS_DIAGNOSTIC_NOT_COUNTED`, 40 проверок из 40 (`s4-38l9j-apic-discovery-verdict.json`; в них 15 обязательных маркеров, 20 запрещённых, включая `Kernel panic` кроме ожидаемой паники `/init`): e820 журнала равен массиву отчёта, ECAM, RSDP, один ЦП, IOAPIC `0xfec00000` GSI 0-23, INT_SRC_OVR, `Using IOAPIC`, TIMER pin 2, `Fast TSC calibration using PIT`, C13 → C14 → C16 → C17, `Run /init`; запрещённых сообщений нет. Пункт (4) диагностически подтверждён живым прогоном, не закрыт (результат не засчитывается). Журнал syscall `/init` (83, 165, 257, 0, 3, 1, 39, 35, 60) и `OD_NONCE=s4-38l9j` — наблюдение, вердиктом не проверяется. (5) **Открыто:** прогон остановился `exit_kind: backend` (retired 2 322 382 342) после паники `Attempted to kill init`: запись `0x10000` (LVT Timer: маска, вектор 0) по `0xfee00320` отвергнута LAPIC как `UnsupportedConfiguration` (RIP `0xffffffff813fb122`); нужен срез, принимающий маскированный LVT Timer с вектором ниже 16 (или решение, что это ошибка модели).
- **Суть и последствия (L8d, исторически):** (1) после checkpoint L8d прогон стоит на `mov [rax+0x10], r9d` (RIP `0xffffffff813f2f9d`, retired 1 792 955 123): запись в IOWIN (`0xfec00010`) отвергнута IOAPIC как `UnsupportedConfiguration`; r9 = `0xa020` (уровень, активный низкий, вектор 0x20, без маски). Скорее всего это SCI на пине 9 (по значению; выбранный IOREGSEL не проверялся): модель IOAPIC не принимает снятие маски у входа без линии (`valid_low`). Нужен срез L8e. (2) У `PitIoapicIrqSource` нет `wake_ns` и нет дедлайна таймера LAPIC: HLT с IF = 1 завершает прогон как `halted`; пробуждение HLT по таймеру LAPIC держится только на владельце часов и в живом прогоне не проверено. (3) UART не подключён к INTIN4. (4) `--user-mode` с `stage4a-apic` отвергается (путь CPL3-IRQ построен под 8259, ISSUE-090). (Закрыто в S4-38L9-um, см. статус выше.)
- **Следующий шаг:** живой прогон `stage4a-apic --user-mode` (user mode под APIC закрыт кодом и синтетикой в S4-38L9-um, пункт (4)); живой прогон `stage4a-apic` с пробуждением по таймеру LAPIC и UART (таймер LAPIC закрыт кодом и синтетикой в S4-38L8i, UART в S4-38L8h, `wake_ns` в L8f).

### ISSUE-105 — S4-38L8d: RDMSR IA32_APIC_BASE (0x1B) даёт `#GP`, ядро ограничивается одним CPU

- **Дата обнаружения:** 2026-11-15, живой прогон S4-38L8d.
- **Компонент:** набор CPL0 (`od-isa`), профиль `X8664V1Stage4AApic`.
- **Владелец:** отдельный срез S4-38 (MSR 0x1B).
- **Статус:** закрыта кодом, узкими тестами и живым прогоном `stage4a-apic` (S4-38L8g, 2026-11-15; ожидает независимого ревью и принятия). Член набора `msr:apic_base` (только профиль `stage4a-apic`): `RDMSR` 0x1B даёт `0xFEE00900` (база, BSP, EN), `WRMSR` принимает только это значение. В живом прогоне (release, `--budget 2600000000`, `--checkpoint-table`) в COM1 нет строк `unchecked MSR access error: RDMSR from 0x1b`, `BSP APIC 0 is not marked in APICBASE MSR`, `[Firmware Bug]: APIC enumeration order`, `Limiting to one CPU`; стоп прежний (`user_entry`, C0-C16, C17 не достигнут). stage4a и его `PASS` S4-39j не затронуты (для него 0x1B остаётся `#GP(0)`). Остаётся для N vCPU (S6-18/S6-21): число CPU в MADT и INIT/SIPI, не этот MSR.
- **Суть и последствия (исторически):** в COM1 появилась строка `unchecked MSR access error: RDMSR from 0x1b at rIP: 0xffffffff81409342 (native_read_msr+0x2/0x50)` (условие решения владельца о добавлении MSR 0x1B выполнено). Далее `CPU topo: Enumerated BSP APIC 0 is not marked in APICBASE MSR` и `[Firmware Bug]: APIC enumeration order not specification compliant`; `CPU topo: Assuming crash kernel. Limiting to one CPU to prevent machine INIT` (ядро ограничивается одним CPU; `smp: Brought up 1 node, 1 CPU`). Прогон идёт дальше (ядро ловит `#GP` через fixup); для N vCPU (S6-18/S6-21) MSR нужен.
- **Следующий шаг:** остаётся N vCPU: MADT с несколькими LAPIC и INIT/SIPI (S6-18/S6-21), сам MSR для одного CPU сделан. Сверка с моделью LAPIC (`set_global_enabled`) не делалась: `WRMSR` 0x1B принимает только константу `0xFEE00900` и состояние LAPIC не меняет (отключение и перенос базы не моделируются). JSON живого прогона не сохранён (отчёт прогона не сохранён в репозитории); при повторе сохранить по образцу `docs/reports/data/stage4/s4-38l8e-sci-pin9.json`.

### ISSUE-104 — остатки S4-22/S4-23 переданы после закрытия с переносом

- **Дата обнаружения:** 2026-11-15, решение владельца о закрытии S4-22 и S4-23 с переносом остатков.
- **Компонент:** `od-machine` (`clock_owner.rs`, `firmware_board.rs`, `linux_direct.rs`), `tools/runner`.
- **Статус:** открыта; перенесено, не дефект.
- **Суть и последствия:** владелец часов подключён к рабочему циклу Linux-платы только как opt-in (`enable_clock_owner`, S4-23j). Зачётный прогон runner'ом флаг не включает, поэтому Linux-путь по умолчанию работает через прежний `advance_standalone`. Не сделано: (1) включение флага в runner и зачётный прогон на реальном ядре — S4-38; (2) Linux-видимые частота и поля времени, тики PM/PIT/RDTSC как поля времени — S4-38; (3) LAPIC-таймер как источник пробуждения из HLT (сейчас вектор LAPIC при простое отвергается) — S4-38; (4) типизированные отказы вместо `FirmwareBoardError::Config` — S4-38; (5) N vCPU, квант, seed и расписание — S6-15.
- **Условия проверки:** по S4-38 — зачётный прогон Linux с `enable_clock_owner` и без него, сравнение трассы и часов.
- **Следующий шаг:** включить флаг в runner (S4-38) и зафиксировать результат; остальное — по плану S4-38 и S6-15.

### ISSUE-103 — S4-31e: `XsdtAddress % 8 == 0` в проверке RSDP не следует из формата

- **Дата обнаружения:** 2026-11-15, ревью S4-31e/g.
- **Компонент:** `firmware/uefi/src/acpi/rsdp.rs` (комментарий к `RsdpError::BadXsdtAddress`, строки ~78–79; проверка, строка ~164).
- **Статус:** открыта; мелкое, не подтверждённый дефект поведения.
- **Суть и последствия:** валидатор отвергает `XsdtAddress`, не кратный 8, а комментарий подаёт это как следствие формата XSDT («an XSDT is an SDT with 64-bit entries»). Требование выравнивания XSDT на 8 байт в прочитанном тексте спецификации ACPI не найдено (не сверено); это собственное правило проекта. Таблицы blob выровнены на 16, поэтому на текущем blob проверка ничего не отвергает; последствие только в том, что комментарий вводит в заблуждение.
- **Основание:** ревью S4-31e/g (Opus), чтение `rsdp.rs`.
- **Следующий шаг:** переписать комментарий как правило проекта либо сверить с ACPI 6.x (RSDP, XSDT) и закрепить ссылку.

### ISSUE-102 — S4-31g: упаковщик ACPI blob падает исключением, а не `BuildError`, на короткой таблице

- **Дата обнаружения:** 2026-11-15, ревью S4-31e/g.
- **Компонент:** `tools/build/build_stage4_acpi_blob.py` (разбор blob, строки ~176–177 и ~218–219).
- **Статус:** открыта; мелкое, на корректном blob не проявляется.
- **Суть и последствия:** если `payload_len < 36`, чтение RSDP даёт `IndexError` (срез короче `RSDP_LEN`); если запись FADT короче 148 байт, `struct.unpack_from("<Q", fadt, 140)` даёт `struct.error`. Вместо контролируемого `BuildError` упаковщик завершается трассировкой. Принять испорченный blob это не позволяет (исключение так же прерывает сборку), страдает только диагностика.
- **Условия обнаружения:** чтение кода; тест на такие входы в `tools/build/test_build_stage4_acpi_blob.py` отсутствует.
- **Следующий шаг:** проверить длины перед чтением и поднимать `BuildError`; добавить тест.

### ISSUE-101 — S4-31g: валидатор FACS в `od-machine` проверял только сигнатуру и Length

- **Дата обнаружения:** 2026-11-15, ревью S4-31e/g.
- **Компонент:** `crates/od-machine/src/acpi_blob.rs` (`validate_acpi_blob`, блок FACS).
- **Статус:** закрыта (в этом же срезе исправлений). Валидатор требует ровно те байты, которые пишет сериализатор прошивки: `FACS`, `Length` 64, версия 2 (байт 32), все прочие байты нулевые. Тест `the_facs_segment_is_checked` проверяет порчу версии и reserved-байта, `every_corrupted_header_field_and_byte_of_the_fixture_is_refused` — порчу каждого из 64 байт FACS.
- **Суть и последствия (до исправления):** испорченные версия и reserved-байты FACS проходили машинную проверку blob, хотя `validate_facs` в прошивке их отвергал.
- **Следующий шаг:** нет.

### ISSUE-100 — S4-31e: отступления FADT/FACS от возможных требований не сверены по первоисточнику

- **Дата обнаружения:** 2026-11-15, ревью S4-31e/g.
- **Компонент:** `firmware/uefi/src/acpi/fadt.rs` (флаги FADT, `FIRMWARE_CTRL`/`X_FIRMWARE_CTRL`), проверка в `crates/od-machine/src/acpi_blob.rs`.
- **Статус:** открыта; два пункта, оба «не сверено», подтверждённого дефекта нет.
- **Суть и последствия:**
  1. Флаги `WBINVD` и `WBINVD_FLUSH` в FADT равны 0 (в комментарии к `FADT_FLAGS` объяснено тем, что они нужны только для состояний сна S1..S4, которых плата не объявляет). Существует ли для них требование «must be set» в ACPI 6.x и применимо ли оно к плате без сна, по первоисточнику не сверено (текст читался только в редакции 6.4, фрагментами).
  2. `FIRMWARE_CTRL` и `X_FIRMWARE_CTRL` оба равны `0xE_F000` (адрес FACS); валидатор требует равенства обоих адресу FACS. Спецификация может требовать, чтобы при ненулевом `X_FIRMWARE_CTRL` значение `FIRMWARE_CTRL` игнорировалось или было нулём; обязательность нуля по первоисточнику не сверена. Linux отдаёт приоритет 64-битному адресу (`acpi_tb_select_address`, `tbfadt.c`), поэтому на Linux различия не ожидается, но принятие таблиц Linux ещё не проверялось.
- **Условия обнаружения:** чтение `fadt.rs` и валидатора ревью; запуск Linux с этими таблицами выполняется в S4-38.
- **Следующий шаг:** сверить оба пункта с ACPI 6.x (Table 5.10, FADT; FACS) и решить вместе с S4-38 (принятие таблиц Linux); при расхождении исправить `fadt.rs`, golden-байты и валидатор.

### ISSUE-099 — S4-39j: правило зачётности `counted` для stage4a принято решением пользователя без записанного ревью

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45.
- **Компонент:** `tools/runner/src/linux_direct.rs` (значение `acceptance`), `docs/specs/CPU_CONTRACT.md` (раздел «Правило зачётности `acceptance` (S4-39j)»).
- **Владелец:** владелец этапа 4 (пользователь); исполнитель — следующий срез гейта (S4-44/S4-46 или этап 5).
- **Статус:** открыта.
- **Суть и последствия:** до решения прогоны `stage4a` были `diagnostic_not_counted`; в день PASS S4-39j пользователь решил считать stage4a зачётной конфигурацией (без `--diagnostic-past-c7`). Независимое ревью этого решения не записано. Профиль stage4a урезан (нет APIC и SSE/SSE2), поэтому `PASS` не означает поддержку Linux в профиле по умолчанию.
- **Следующий шаг:** записать ревью решения о зачётности либо пересмотреть его; указывать в итогах, что `PASS` относится к stage4a.

### ISSUE-098 — S4-39j: гейт не проверяет `exit_kind`, `cpuid_profile` и timer IRQ

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45.
- **Компонент:** `tools/runner/src/linux_gate.rs`.
- **Владелец:** владелец гейта S4-39j; срез не назначен (кандидат — S4-44 или этап 5).
- **Статус:** открыта.
- **Суть и последствия:** гейт принимает `budget_exhausted` как конец прогона (строка паники и журнал syscall), а не `user_exit`; `exit_kind` и `cpuid_profile` не проверяются, `acceptance` берётся из отчёта; работающий timer interrupt и продолжение user code после него не проверяются. Отчёт, самосогласованный по JSON, но с иным профилем или причиной останова, не отвергается. Пробелы негативов S4-40: отключённая IRQ-маршрутизация, запрещённый overlap, порча артефакта кроме хешей.
- **Следующий шаг:** добавить проверки `exit_kind`, `cpuid_profile` и timer evidence либо явно ограничить гейт; добавить негативы S4-40.

### ISSUE-097 — S4-44: Mac mini bare-metal не выполнен (KVM-прогон выполнен)

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45.
- **Компонент:** `docs/testing/MACHINE.md` (S4-44), `docs/reports/data/stage4/s4-44b-local-set.json`.
- **Владелец:** владелец стенда Mac mini и hardware gate; S4-44.
- **Статус:** открыта для bare-metal (blocked); KVM-часть выполнена.
- **Суть и последствия:** KVM-прогоны на Mini выполнены (S4-34a, S4-34b, S4-34cde; VT-x guest execution под Linux/KVM, не bare-metal, наш CPU/MMU не участвует). Bare-metal hardware cases не выполнены (стенд и владелец). В локальный набор S4-44 hardware не входит.
- **Следующий шаг:** получить стенд и выполнить выбранные bare-metal cases в рамках S4-44; до этого не называть KVM-прогон физическим hardware-подтверждением нашего CPU.

### ISSUE-096 — S4-43/S4-44: лимиты ресурсов и stress не реализованы

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45 (передача остатков S4-43).
- **Компонент:** `tests/system/` (runner stress mode), `crates/od-jit/src/dispatch.rs` (executable cache), runner (`--reboot-cycles`), guest RAM.
- **Владелец:** S4-44; далее `docs/plans/STAGE_7.md` (S7-26) для boot/STOP/reset и `--reboot-cycles`.
- **Статус:** открыта.
- **Суть и последствия:** не сделаны runner stress mode, `--reboot-cycles`, явный лимит guest RAM (проверены только host heap/RSS, RAM фиксирована 1 MiB в тестах S4-43; Linux-плата использует 128 MiB) и лимит executable cache. Повторные STOP/boot в runner, timer reprogramming и старые IRQ/handles после reset не проверены. Все пункты записаны в NOT RUN сводки S4-44b.
- **Следующий шаг:** S4-44 назначает исполнителя по каждому пункту; остаток до STAGE_7.

### ISSUE-095 — Linux direct boot не запускался с JIT (0 %)

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45.
- **Компонент:** `crates/od-jit/`, `od-runner linux-direct`.
- **Владелец:** S4-42 (остаток: JIT cold/warm на одинаково поддержанном сценарии).
- **Статус:** открыта.
- **Суть и последствия:** все Linux-прогоны идут на интерпретаторе; JIT memory integration на реальном ядре и сравнение interpreter/JIT не выполнены. Ограничение S4-39 «JIT coverage указывается отдельно» выполнено указанием 0 %.
- **Следующий шаг:** срез S4-42 для JIT на одинаково поддержанном сценарии либо перенос на этап 5.

### ISSUE-094 — S4-41: живой serial и доставка `od-runner` в бандле не сделаны

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45.
- **Компонент:** `apps/vmhost/Sources/LinuxDirectRunner.swift`, бандл приложения.
- **Владелец:** S4-41c (расширение протокола; не выбран).
- **Статус:** открыта.
- **Суть и последствия:** serial и статус во время прогона не выводятся (только итоговый JSON-отчёт); `od-runner` не доставляется рядом с VMHost; реальный прогон ядра через VMHost не запускался; утечки RAM и handles внутри раннера не проверялись.
- **Следующий шаг:** решение по S4-41c; до него VMHost-запуск Linux не считать работоспособным.

### ISSUE-093 — control v2 не запускает Linux-плату; pause/resume и STOP для неё отсутствуют

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45 (по S4-41b).
- **Компонент:** control v2 (`ControlV2Configure.validate`, `ControlV2Connection`), `od-runner linux-direct`.
- **Владелец:** S4-41c (расширение протокола; не выбран).
- **Статус:** открыта.
- **Суть и последствия:** CONFIGURE не несёт путь к ядру и принимает только профили 1 и 4, поэтому Linux-профиль недостижим по протоколу; pause/resume возвращают `unsupported` (нет асинхронной паузы); STOP по control v2 для Linux-платы не подключён.
- **Следующий шаг:** решение владельца о расширении протокола (с версией ABI и единым нормативным источником) или об отказе от него.

### ISSUE-092 — nonce S4-39j фиксированный (`s4-38l4a`)

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45.
- **Компонент:** `tools/runner/src/linux_direct.rs`, `tools/runner/src/linux_gate.rs`, прогоны S4-39j.
- **Владелец:** S4-46/S4-44 (новый nonce на каждый прогон).
- **Статус:** открыта.
- **Суть и последствия:** план S4-39 требует новый nonce; оба холодных прогона и `PASS` S4-39j использовали `s4-38l4a`. Фиксированный nonce не позволяет отличить новый запуск от повторного использования старого вывода: гейт сверяет nonce с отчётом, но не свежесть. Smoke-серия со свежим nonce для Stage 5 описана в ISSUE-081 и закрытием S4-39j не является.
- **Следующий шаг:** запускать S4-39j с новым nonce на прогон и фиксировать его в отчёте; повторить два холодных прогона.

### ISSUE-091 — профиль stage4a без APIC и SSE/SSE2 (Linux на 8259)

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45.
- **Компонент:** CPUID-профили `X8664V1Stage4A` и `X8664V1Stage4AApic`, плата Linux direct-boot.
- **Владелец:** этап 5.
- **Статус:** открыта, частично подвинута (S4-38L8d, 2026-11-15).
- **Суть и последствия:** `PASS` S4-39j получен под профилем `stage4a`, где APIC и SSE/SSE2 в CPUID не объявлены, прерывания идут через 8259 PIC; профиль не изменён. S4-38L8d добавил отдельный профиль `stage4a-apic` (флаг раннера `--cpuid-profile stage4a-apic`): APIC объявлен, таймер идёт PIT → IOAPIC INTIN2 → LAPIC, включён владелец часов. Под ним достигнут checkpoint C13 (`check_timer`, путь pin 2 без отказных сообщений, 401 IRQ), но прогон не доходит до userspace (ISSUE-105, ISSUE-106), результат не засчитывается, `PASS` S4-39j остаётся за `stage4a`. SSE/SSE2 по-прежнему не объявлены ни в одном профиле. Работа Linux с APIC в профиле по умолчанию не подтверждена.
- **Статус S4-38L9J (2026-11-15):** discovery на APIC диагностически подтверждён вердиктом S4-38L9J (`s4-38l9j-apic-discovery-verdict.json`, `PASS_DIAGNOSTIC_NOT_COUNTED`, 40 проверок), не закрыт: под `stage4a-apic --user-mode` Linux находит e820, ECAM, RSDP, IOAPIC и таймер на pin 2, доходит до `/init` (C17); журнал его syscall записан как наблюдение (вердиктом не проверяется). Это диагностика: профиль не засчитан, прогон остановился записью LVT Timer после паники (ISSUE-106, пункт 5), счётное evidence под APIC и SSE/SSE2 открыто.
- **Следующий шаг:** L8e и последующие срезы S4-38 (ISSUE-105, ISSUE-106); этап 5 решает, нужны ли SSE/SSE2, и повторяет S4-39j под профилем с APIC.

<a id="issue-090-внешнее-irq-из-cpl3-не-поддержано-run_failed"></a>

### ISSUE-090 — CPL3 external IRQ did not resume through the board loop

- **Дата обнаружения:** 2026-11-15, итоговое ревью S4-45 (по S4-39h).
- **Компонент:** `run_with_cpl3_profile_set` (`crates/od-isa`), плата Linux direct-boot с `--user-mode`.
- **Владелец:** этап 5 / prerequisite S4-39h.
- **Статус:** resolved for the modeled CPL3 interrupt path; live guest/stand validation remains open under S4-39 and ISSUE-081.
- **Суть и последствия:** CPL3 external IRQ delivery now validates the current ring-3 state, IDT gate and target descriptor, reads `TSS.RSP0`, writes the privilege-switch frame with CPL0 page privilege, nulls SS, enters the handler, and returns through IRETQ. The board loop now restarts its boundary after a successful delivery so it recalculates CPL/context before executing the handler. A focused board integration test verifies IRQ delivery and user-code continuation. This resolves the modeled `run_failed` gap; it does not attribute the older S5-07 C/run-0001 failure, which lacked CPL/RIP/refusal evidence.
- **Результат проверки:** independent code review clean; `cargo test -p od-isa --test cpl0_profile_set --offline` (1592 passed), `cargo test -p od-machine --lib linux_direct::tests::irq_hook --offline` (17 passed), focused formatting check passed.
- **Ограничение и следующий шаг:** no live Linux guest or physical stand run has exercised the path. Rebuild the runner and repeat the common S5-07 series before closing S5-07 or the parent S4-39 gate.

<a id="issue-081-s5-07-series-evidence-prepared-but-stand-blocked"></a>

### ISSUE-081 — S5-07: серия и evidence schema подготовлены, но стенд заблокирован

- **Дата обнаружения:** 2026-11-12, при подготовке orchestration для `linux-direct`.
- **Компонент:** `crates/od-machine/src/linux_direct.rs`, `tools/runner/src/linux_direct.rs`, `tools/runner/src/linux_direct/series.rs`, Stage 5 run evidence и `--compare-results`.
- **Владелец:** Stage 5 runner; принятый S5-01 Method gate задаёт процедуру и критерии; Numeric gate определяет `N` и soak budget только для S5-15/16.
- **Статус:** открыта; per-run PM/RDTSC/PIT telemetry и `rdtsc_reads_by_cpl` прошли независимое ревью; focused machine/runner tests подтверждают per-invocation deltas, CPL classification и strict JSON fields. Последняя automatic N=2 серия `stage5-s5-07-stage4a-rdtsc-by-cpl-20261115-f` прошла оба scenario и nonce markers, но comparator вернул `matches=false` по `machine_observations` и `ordered_uart_bytes`. Все RDTSC reads были CPL0; их разница 39,025 арифметически совпадает с 39,025,000 нс residual delta. PIT/PM counts совпали, а delivered `#PF` — 35/33. CPL3 retired/syscall journal, `cr3_changes` и конечный set exit совпали; сырые IRQ/quanta observations различаются, но существующая нормализация comparator их исключает. Machine mismatch остаётся по новым strict counters/PF counts; в UART audit init line сместилась через пять thermal governor lines, а terminal panic snapshots содержат разные optional frames, не все разрешены текущим строгим normalizer. Причины не установлены; новые fields и UART различия не ослаблены. S5-07 остаётся `blocked/unaccepted`, checkbox пустой. Numeric gate S5-01 не блокирует S5-07. Подробные метрики и hashes: [RDTSC-by-CPL N=2 evidence](reports/data/stage5/s5-07-rdtsc-by-cpl-n2-20261115.log); предыдущие [source-count](reports/data/stage5/s5-07-source-read-n2-20261115.log) и [PM-count](reports/data/stage5/s5-07-pm-timer-read-n2-20261115.log) серии.
- **Основание и последствия:** добавлен явный smoke-series path с per-run nonce, collision-refusing evidence directories, raw runner JSON, stop record, runner-recorded COM1 bytes/hash/count, manifests и ordered marker validation. Fixture comparator сохранён; `--linux-stage5` добавляет schema comparison. Stop record теперь переносит типизированную причину ACPI PM (`reset`/`shutdown`/`null`) из `FirmwareRunReport`; отсутствующее поле в старом JSON трактуется как ненаблюдавшееся. Series повторно выводит поле из raw JSON, проверяет целостность и сравнивает причины отдельно от `exit_kind`; наличие причины само по себе не ставит сценарий в pass. Синтетические и runner tests проверяют передачу/сравнение. Выполнен один live `smoke_not_acceptance` run с nonce `OD518e74b980609c9e800003ecc00000000`, budget 1,200,000,000 и `diagnostic_past_c7=false`: `guest_started=true`, ожидаемый и фактический stop — `cpuid_divergence` leaf `0x1`, `retired=1,137,476,410`, wall elapsed 422.901293667 s, `machine_stop_reason=null`. COM1 содержал 0 bytes, exact `OD_NONCE=<nonce>` marker отсутствовал; поэтому scenario/series outcome `failed`, `/init` не достигнут, S5-07 не принят. Host из manifest — macOS/aarch64, runner 0.1.0, source revision отсутствует. Kernel из sibling main worktree, прочитан read-only, SHA-256 `9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e`; initramfs оттуда же, SHA-256 `ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588`; ACPI blob из codex worktree, SHA-256 `b1ba6078a4d9bc7a8d224980713d8242e4a708743ce4f56093b00a07c631ffaf`, manifest hash `2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2`. Raw evidence в ignored `work/stage5/stage4-c7-live-20261112-02/`: series SHA-256 `080108859b67a964d3c1eec8247b6d8155f01ef1e18ad7aef1d89a916661d194`, run manifest SHA-256 `d708f5f57cd71c795a10d51443a72a42ac2767f190e42ec5d16c9e30f69f3962`, raw runner JSON SHA-256 `6bff397201eaafc0836aeb6dbddfa6577d4844cc4601c5680ed827b3d51eeb7c`. Предыдущий debug run `stage4-c7-live-20261112-01` был технически отменён SIGINT (exit 130) после 20 минут CPU-bound вычисления до записи run evidence; это не сценарный отказ. Текущий COM1 источник — stop-scoped `linux_direct_run_boot_com1`, а не независимая streaming UART capture. `loaded` не означает guest scenario pass; Stage 5 A–G остаются `not_run`, поскольку scripts ещё не вызываются boot runner. Отсутствие host/user CPU measurements, `N` или soak budget относится к Numeric gate для S5-15/16, а не к блокеру S5-07.
- **Диагностический срез (2026-11-15):** `FirmwareBoardError::ExternalInterruptDelivery` передаёт vector, Debug-представление typed refusal, `RIP` и `CS selector` в `run_failed`; unit и facade tests это проверяют. Свежая counted N=2 серия на этой сборке прошла без IRQ отказа, так что ветка диагностики не сработала и старый C/run-0001 не локализован. Строгий comparator новой серии с C вернул `matches=false`; старый aggregate `failed` сохранён. Подробные данные: [diagnostic evidence log](reports/data/stage5/s5-07-irq-diagnostic-20261115.log).
- **Повтор после S4-39h (2026-11-15):** обе counted runs прошли ожидаемый stop и exact nonce, но внутренний strict comparator вернул `matches=false` по `machine_observations` и `ordered_uart_bytes`. Counts `exceptions_delivered` означают успешные guest exception deliveries (`#PF` и `#GP(0)`): 35 против 34; vector breakdown не сохранён, оснований объявлять вариацию безвредной пока нет. Та же внутренняя comparator-ошибка наблюдается у pre-fix diagnostic N=2, поэтому её нельзя приписать S4-39h. Доказательство и hashes в [post-fix N=2 log](reports/data/stage5/s5-07-post-cpl3-irq-n2-20261115.log).
- **Диагностический срез (2026-11-15):** stop record и raw runner JSON добавлены отдельные счётчики successful `#PF` и `#GP(0)` delivery; старый `exceptions_delivered` сохраняется как агрегат. Отдельные machine tests проверяют оба вектора и нулевое значение на следующем вызове; новый live N=2 подтвердил счётчики `33 #PF + 1 #GP(0)` в каждом run.
- **Диагностическая пара (2026-11-15):** два независимых counted single runs с новым sidecar локализовали большую часть RDTSC variation в `delay_tsc+0x4d` (34,062,756/34,054,713 reads, delta −8,043); total delta −7,975 объясняет residual virtual-time delta арифметически. Delivered `#PF` — 33/34; все события совпадают по порядку, кроме единственной вставки во втором run после 30 общих records и перед общим CPL3 fetch fault: CPL0 byte store в `rep_movs_alternative+0x11` по user-stack VA `0x7ffc659bc779`. Причина page fault не установлена. Exact nonce markers и CPL3/syscall work совпали. Run/init printk и nonce меняют порядок; terminal panic optional frames также отличаются. Это localization, не доказательство безвредности; N=2 comparator не запускался и S5-07 остаётся blocked. Полные hashes и CLI: [execution-observations diagnostic pair](reports/data/stage5/s5-07-execution-observations-pair-20261115.log).
- **Диагностическое расширение (2026-11-15):** execution sidecar schema v2 добавляет для каждого успешно доставленного #PF fault-state `CR3/RSP/RBP/RCX/RSI/RDI`, снятый до `deliver_page_fault_cpl0_set`; CPL3 synthetic test подтверждает, что это user `RSP`, а не handler `RSP0`. CPL0/CPL3 machine tests, serializer test для всех полей и offline checks прошли; `cargo check -p od-machine -p od-runner --all-targets --offline` зелёный с существующим unrelated `page_walk.rs` dead-code warning. На момент этого implementation slice нового schema-v2 live run ещё не было; предыдущие raw sidecars остаются schema v1.
- **Fault-context pair (2026-11-15):** две новые counted single runs с schema v2 обе содержали 34 delivered #PF. Event `rep_movs_alternative+0x11` в обоих — CPL0 byte write, `RCX=7`, `RDI=CR2`, `RSI` указывает на `x86_64\0` в `.rodata`; kernel RSP/RBP совпадают, адрес назначения на `0x1b9` выше следующего CPL3 user RSP. Это согласуется с копированием семибайтовой строки на startup stack и возможным first-touch fault, но точный caller/PTE transition не сняты; более ранняя пара имела 33/34 событий. RDTSC totals отличаются на 30,074; comparator не запускался, benignity не заявлена, blocker сохраняется. Подробности и hashes: [fault-context pair](reports/data/stage5/s5-07-fault-context-pair-20261115.log).
- **Диагностический срез frame-chain (2026-11-15):** sidecar schema v3 теперь хранит до 32 frame-pointer links при успешной #PF delivery; цепочка читается до delivery из ограниченного 4 KiB диапазона через read-only RAM walks без guest A/D writes и только при opt-in. Два synthetic machine frames и null termination, non-RAM refusal regression, ограничители helper, serializer и scoped offline checks прошли; независимое ревью завершилось PASS. На момент этого code slice live schema-v3 pair ещё не выполнялась; её результат записан ниже. S5-07 остаётся blocked/unaccepted.
- **Диагностическая пара frame-chain (2026-11-15):** две counted single runs дали 34/33 delivered #PF без sidecar omissions. Первые 30 fault signatures совпали; в run `k` есть одна дополнительная CPL0 byte write в `rep_movs_alternative+0x11`, затем suffix signatures совмещается с run `l`. Её 10 captured returns разрешаются matching debug ELF от `create_elf_tables` через `load_elf_binary`/inline `search_binary_handler` в `exec_binprm`, `bprm_execve`, `kernel_execve` и `kernel_init`, локализуя событие в ELF/initial-stack setup; это поддерживает startup-string-copy гипотезу, но не доказывает причину optional fault или PTE transition. Из RDTSC delta 3,737 — 3,647 чтений на `delay_tsc+0x4d`; причина разного числа итераций остаётся не установлена. IRQ0 edges/deliveries различаются на четыре; comparator не запускался, benignity не заявлена. [Полные данные и hashes](reports/data/stage5/s5-07-fault-frame-chain-pair-20261115.log). S5-07 остаётся blocked/unaccepted.
- **Диагностический срез original page-walk (2026-11-15):** sidecar schema v4 сохраняет до четырёх raw `WalkEntry` из первоначального гостевого walk и `PageFaultReason`, только при opt-in и только для `PageFault`; обычный доступ копирует `TrackedAccessReport.trace`, split read/write preflight выставляют уже произведённый trace без повторного чтения таблиц. Синтетические тесты подтверждают отсутствующий PDE со значением 0 после исправления handler'ом, trace неудачной второй страницы, корректные A/D эффекты, отсутствие operand write при отказе preflight и отсутствие sidecar trace без opt-in. Runner serialization проверяет reason, уровни/адреса/значения записей и предел. Две live schema-v4 counted single runs завершились с 34 #PF без omissions; совпадающее событие в `rep_movs_alternative+0x11` показывает `NotPresent { level: Pt }` на исходном guest walk и 10-frame caller chain в ELF initial-stack path. Это доказывает исходную нулевую PT leaf только до delivery, не изменение PTE после обработки. По серии schema v1–v4 событие было в 6 из 8 runs; автоматического comparator на этой паре не было.
- **Диагностический implementation slice post-IRET (2026-11-15, code-only):** schema v5 добавляет opt-in `--execution-observations-post-iret-index <n>` для выбранного delivered #PF. Захват ждёт успешный IRETQ с совпадающим RIP/CS/CPL/RSP, сохраняет текущий CR3 и без A/D обновлений снимает до четырёх 4 KiB/2 MiB page-walk entries по сохранённому CR2/access. Статусы явно различают `event_not_seen`, `return_not_observed`, `context_mismatch` и `captured`; nested delivery depth удерживает IRQ/#PF/#GP returns от ложного завершения или mismatch выбранного события, а series mode отклоняет флаг. Синтетические machine tests покрывают repaired leaf, неизменные RAM/A-D callbacks, 2 MiB leaf, permission fault и чужой nested return; runner tests покрывают схему и CLI gates. Scoped offline checks и focused tests прошли. На момент реализации schema-v5 live evidence ещё не было; последующая диагностическая пара в этом ISSUE подтвердила mapping на границе возврата IRETQ. Этот snapshot не доказывает последующую инструкцию или TLB behavior.
- **Диагностическая пара post-IRET page-walk (2026-11-15):** два отдельных counted single runs на одинаковых input hashes и schema v5 захватили ordinal 30. В обоих исходный CPL3 fetch по RIP/CR2 `0x4000b0` завершился с нулевым PML4 entry; после соответствующего IRETQ на том же CR3 walk перевёл адрес в `0x443d0b0` через PTE `0x443d025`. Mapping transition между failed access и return boundary тем самым наблюдался повторно. Runner stop records совпали по 33 delivered `#PF`, 1 `#GP(0)` и budget stop; IRQ0 differed by 2 edges/deliveries, RDTSC reads by 1,510, quanta by 1,658, virtual time by 1,695,503 ns. Динамические CR3 table addresses и user stack pointers различались. Это диагностические одиночные runs, не automatic N=2; strict comparator не запускался, timing differences не нормализованы и benignity не заявлена. Event-absence variation в более широком наборе и ранее наблюдавшийся источник timing variance `delay_tsc+0x4d` остаются открытыми. [Evidence log](reports/data/stage5/s5-07-post-iret-page-walk-pair-20261115.log).

- **Vector-count повтор (2026-11-15):** новый N=2 на `2ec744efdb2be75ccd934e3927129db86ea6189c` дал в обоих запусках 33 `#PF` + 1 `#GP(0)` (aggregate 34), так что прежняя разница aggregate exception count не повторилась. Первичная проверка сравнения вернула `matches=false` по machine observations и UART. Первый terminal panic trace содержит `? __task_pid_nr_ns+0x6c/0x120`, отсутствовавший в тогдашнем optional-frame allowlist; из-за этого `stage4_nonce_variation_eligible` выключил volatile IRQ/terminal normalization. IRQ/IRQ0 counters различаются на 4 (34,821/34,816 против 34,825/34,820). Последующий узкий допуск этого optional frame объяснил machine observations, но оставил UART difference. Полные hashes — в [vector-count evidence log](reports/data/stage5/s5-07-stage4a-vector-counts-20261115.log); отчёт: [S5-07 follow-up](reports/STAGE_5_REPORT.md#s5-07-vector-count-n2-follow-up-2026-11-15).

- **Comparator follow-up (2026-11-15):** после commit `3d15e1a` release comparator пересравнил те же raw run artifacts; новый Linux run не выполнялся. Результат `matches=false`: machine observations теперь в `explained_differences`, единственный unexplained difference — `ordered_uart_bytes`. В run 0001 `OD_NONCE=...` следует перед `Run /init as init process`; в run 0002 эти две строки идут в обратном порядке. Также встроенное поле audit timestamp меняется `audit(0.140:1)` → `audit(0.141:1)`, а текущая нормализация обрабатывает только внешний printk timestamp. Порядок marker и embedded audit time оставлены строгими, поскольку их допустимость не доказана. Причина расхождения UART остаётся открытой; подробности — в [vector-count evidence log](reports/data/stage5/s5-07-stage4a-vector-counts-20261115.log).

- **UART stack-context diagnostic (2026-11-15):** код прошёл независимое ревью без блокеров и был проверен на двух отдельных live single runs (это не counted series). Каждый run достиг ожидаемого budget stop, nonce marker встретился ровно один раз, sidecar записал четыре snapshot. Все 10 reliable symbol+offset frames каждого snapshot совпали с цепочкой frame-pointer addresses и разрешились debug ELF, чей `.text` совпадает с kernel payload. Optional `?` candidates в этих sampled chains не встретились; это не доказывает, что optional frames в целом недопустимы. Два запуска по-прежнему различаются на один успешный `#PF`, пять IRQ0 edges и пять total IRQ deliveries, а также 28 UART bytes; comparator на паре не запускался, поэтому причина strict N=2 variation не установлена. Снимки — только `console-output context` (`unwind_context=false`), wall-time overhead не измерен. Подробности и artifact hashes: [console-context N=2 evidence log](reports/data/stage5/s5-07-console-context-n2-20261115.log).

- **UART byte-provenance live follow-up (2026-11-15):** два отдельных single-run schema v2 capture сохранили по 512 байтов начиная с `=` после `OD_NONCE`, и окна побайтно совпали с raw transcript. Стековые сэмплы указывают на `serial8250_tx_chars` / IRQ handler для nonce и `serial8250_console_wait_putchar` / deferred printk для следующей строки `Run /init as init process`; это наблюдение разных writer paths, а не установленная причина старой склейки. В отдельной свежей automatic N=2 серии обе точные nonce-строки валидны, но comparator вернул `matches=false`: меняются machine stop observations и порядок двух целых UART строк. IRQ0 edges 34,794/34,824, delivered IRQs 34,799/34,829, quanta 34,560,550/34,589,454; 34 `#PF` + 1 `#GP(0)` в каждом. Строгая нормализация не менялась; S5-07 остаётся blocked/unaccepted. Raw hashes и команда: [UART provenance evidence log](reports/data/stage5/s5-07-uart-provenance-20261115.log). Следующий шаг — установить источник scheduler/IRQ и UART-order variation и получить strict-clean N=2; не маскировать эти различия.

- **Current-source automatic retry (2026-11-15):** перед новым run current comparator повторно сравнил сохранённую vector-count N=2 пару: `matches=true`, `unexplained_differences=[]`; output hash `3fa9e27b9dd0bc053faa18f08ffab2b3874301434623598b18561eb3fe64f105`. Свежая автоматическая серия `stage5-s5-07-stage4a-auto-compare-20261115-b` на clean source revision `61d6a6590e28a5619637bf70515335eafe48e2e1` завершилась `failed`. Run 0001 прошёл exact marker; run 0002 достиг `budget_exhausted`, но UART stream содержит `OD_NONCE=OD518e830aea36265000000b50e00000001[    2.271034] Run /init as init process` без newline сразу после nonce. В fixture `init.S` nonce и newline передаются одним `SYS_write`; причина interleaving в захваченном COM1 потоке не локализована. Marker validation отказала, поэтому comparator вернул четыре unexplained differences (scenario status, marker validation, machine observations, ordered UART). Полные hashes и command — в [current-source N=2 evidence log](reports/data/stage5/s5-07-current-source-n2-20261115.log).

- **Обновление (2026-11-14):** новые runner builds фиксируют Git revision и dirty state production source inputs, а S5 series writer выдаёт schema v2; reader сохраняет совместимость с v1, где отсутствующее `source_dirty` остаётся unobserved. Dirty bool не является fingerprint исходников. Live guest run по-прежнему не имеет nonce marker; это обновление не снимает блокировку S5-07.

- **Обновление (2026-11-15):** counted серии A и B (по одному run, одинаковые Stage 4 artifact hashes) прошли: обе имеют точную `OD_NONCE`, `guest_started=true`, `budget_exhausted`, `retired=2,999,999,998`; comparator A/B вернул `matches=true`, `unexplained_differences=[]`. Затем выполнена общая серия C с `requested_runs=2`: run 1 завершился `failed`/`run_failed` при `retired=2,320,063,581` с ошибкой `an external interrupt of the board source was refused by the CPU delivery`, без stop record и UART bytes; run 2 прошёл с новым nonce и ожидаемым stop. Aggregate series outcome `failed`, поэтому S5-07 остаётся blocked/unaccepted. Независимый разбор подтвердил наблюдаемый отказ, но не установил его точную причину: эта сборка runner не сохраняла vector, refusal detail или CPL/RIP. Позднее Stage 4 review добавил [ISSUE-090](#issue-090-внешнее-irq-из-cpl3-не-поддержано-run_failed), описывающую общий CPL3 IRQ gap; это согласуется с гипотезой, но не доказывает CPL3 для данного run. `ISSUE-088` о syscall queue не относится к этому событию. Подробные hashes и comparator — в [S5-07 evidence log](reports/data/stage5/s5-07-stage4a-nonce-20261114.log); полная граница и вывод — в [отчёте этапа](reports/STAGE_5_REPORT.md#s5-07-counted-nonce-series-follow-up-2026-11-15).

- **CLI orchestration update (2026-11-15):** series command now compares every N≥2 run against the first with the existing strict `compare_run`, saves `comparison.json`, and maps unexplained differences to failure; N=1 reports that within-series comparison was skipped. Unit coverage checks clean N=2 and detects an artifact hash change in run 3. Independent review approved the diff; formatter and all 25 `linux_direct::series::tests` pass. A new release N=2 invocation confirmed sidecar persistence and automatic comparison in one command. Both guest runs passed with exact nonce and expected budget stop, but strict comparison remained `matches=false` on `machine_observations` and `ordered_uart_bytes`. Across saved N=2 traces, optional frame symbols vary (scheduler/PSI, page-fault/mm and task-id paths). Upstream [Linux x86 stack documentation](https://kernel.org/doc/html/v6.6/arch/x86/kernel-stacks.html#printing-backtraces-on-x86) says `?` marks an address outside the expected frame-pointer chain; it is often stale stack data but may also be a real frame when the frame pointer is not set up. Thus blanket-dropping `?` lines is not justified. Raw-stop audit for the latest pair found 23 changed leaf paths: two UART payload fields and 21 machine-observation paths. All 21 machine paths are covered by the existing volatile-observation field set; no other machine-projection fields differ. The machine difference remains because the rejected panic snapshot disables that comparison gate; this locates the comparator rejection but does not prove benignity. UART payload difference remains strict. No comparator rules changed. S5-07 remains open pending explanation and a strict-clean N=2 result. Full command and hashes: [automatic series evidence log](reports/data/stage5/s5-07-auto-series-cli-20261115.log); raw artifacts are ignored under `work/stage5/stage5-s5-07-stage4a-auto-compare-20261115-a/`. Prior failed common-ID evidence remains unchanged.

- Диагностический срез и два live single-run результата зафиксированы выше; ограничения comparator и блокировка S5-07 остаются в силе.


- **Повтор post-IRET capture startup-stack write (2026-11-15):** два независимых counted schema-v5 single runs на одинаковых kernel/initramfs/ACPI/runner hashes выбрали ordinal 30 в `rep_movs_alternative+0x11`. В обоих первоначальный PT leaf был нулевым; после IRETQ matching CPL0/RIP/CS/RSP на том же CR3 наблюдал leaf `0x39fe867` и translation saved CR2. Это подтверждает mapping transition для двух разных user-stack адресов, но не retirement повторного REP и не TLB. В этой паре C/D fault-signature последовательность различалась одним CPL0 write `#PF`: только C имел ordinal 31 по RIP `0xffffffff8271ec4d`, CR2 `0x7ffc939c3fb0`, с нулевым PT leaf; staged debug ELF разрешил RIP в `__put_user_8` (`putuser.S:115`). D не имеет matching signature. Это объясняет разность 35/34 #PF только в этой паре; причина, post-IRET mapping ordinal 31, ASLR и benignity не доказаны и другая acceptance-пара здесь не анализировалась. S5-07 остаётся blocked/unaccepted. [Paired evidence](reports/data/stage5/s5-07-post-iret-stack-store-pair-20261115.log).

- **`norandmaps` diagnostic N=2 (2026-11-15):** commit `886f7fa` added opt-in `--linux-norandmaps`, gated to the full Stage 4A user-mode profile and marked `diagnostic_not_counted`; independent review passed. A fresh automatic N=2 diagnostic reached the expected budget stop and exact nonce in both runs. Aggregate delivered `#PF` counts matched at 33/33, but the series collected no per-event sidecars, so it does not show whether the earlier startup-stack store fault was absent or establish ASLR as its cause. Strict comparison still returned `matches=false` for machine observations and nonce-normalized UART bytes; RDTSC differs by 16,911 (CPL0), IRQ0 edges/deliveries by 17 and quanta by 16,862. Embedded audit ordering, `sched_clock` calibration and terminal optional-frame differences remain strict. No normalizer changed; S5-07 remains blocked/unaccepted. [Evidence log](reports/data/stage5/s5-07-norandmaps-n2-20261115.log).

- **`norandmaps` sidecar pair plus matched control (2026-11-15):** два schema-v5 treatment runs при одинаковых kernel/initramfs/ACPI inputs имеют одинаковые полные fault signatures — по 33 `#PF`, без omissions; в обоих отсутствуют known startup-stack store RIP `0xffffffff8271a9c1` (`rep_movs_alternative+0x11`) и `0xffffffff8271ec4d` (`__put_user_8`). Ordinal 30 — одинаковый CPL3 fetch fault `0x4000b0`, post-IRET walk переводит его через leaf `0x443d025`. Добавленный same-binary control без `norandmaps` имеет такую же 114-byte cmdline и одинаковые guest input hashes; более длинный nonce компенсирует отсутствующий параметр, поэтому token value различается. В этой одиночной control run stack-store fault вернулся на ordinal 30; после IRETQ leaf `0x39fe867` переводит fault address. Это прямое evidence связи события с `norandmaps` intervention в данном профиле, но control — один run; общая benignity и acceptance не установлены. Автоматический strict N=2 по-прежнему расходится по machine/timing observations и UART; нормализатор не менялся. [Sidecar and matched-control evidence](reports/data/stage5/s5-07-norandmaps-post-iret-pair-20261115.log).

- **RDTSC localization in the same norandmaps pair:** sidecars recorded all 22 CPL0 sites with no overflow or unattributed reads. Total differed by 6,976; 6,905 came from 0xffffffff8271b34d, resolved through the matching debug ELF to rdtsc_ordered in Linux delay_tsc (arch/x86/lib/delay.c:72). The staged source shows this loop continues until the requested TSC interval elapses, but actual elapsed-cycle values were not captured. Cause and benignity remain unproved; comparator counts stay strict.

- **Same-input replay control (2026-11-16):** two separate direct single runs with the same nonce and identical guest inputs produced byte-identical UART, identical complete stop/machine observations and byte-identical schema-v5 sidecars. Both reached the same budget stop and exact marker. This pair had no run-to-run variation on the current fixture, but the prior RDTSC N=2 evidence used ACPI blob `b1ba6078…` while this replay used the current 1104-byte CPU0 blob `6fc25b09…`; it therefore does not isolate nonce or reproduce the prior artifact set. It does not prove nonce changes caused earlier variation or exclude guest-internal randomness. The duplicate-nonce pair is diagnostic only, not S5-07 acceptance; strict unique-nonce N=2 and explanation of earlier differences remain required. [Evidence log](reports/data/stage5/s5-07-identical-input-replay-20261116.log).

- **Current CPU0 ACPI automatic N=2 (2026-11-16):** both generated unique nonces reached `/init`, passed exact marker validation and stopped at the 3-billion budget. The automatic comparator returned `matches=false`: terminal panic snapshots contain unallowlisted optional frames (`? common_interrupt` in run 1; `? console_srcu_read_unlock` and `? tty_ldisc_deref` in run 2), so the Stage 4 nonce eligibility gate rejected this pair and the volatile-field projection was not applied. The comparator reports 29 raw machine leaf differences, including CR3/GPR/terminal registers, IRQ/timing counters, `cr3_changes` 26620/26618 and CPL0 RDTSC counts 34428852/34409309; ordered UART also differs after nonce normalization. `#PF` and `#GP` counts match (33/33 and 1/1). No event sidecars were collected. This confirms strict variation with the current 1104-byte CPU0 ACPI blob but does not establish nonce causality. S5-07 remains blocked pending strict-clean N=2, admissible terminal panic context and localization of RDTSC/UART variation. [Evidence log](reports/data/stage5/s5-07-current-acpi-n2-20261116.log).
- **Series sidecar capture implementation (2026-11-16):** reviewed runner change adds opt-in `--execution-observations`; each series run receives its own schema-v5 observation sidecar. Preflight validates the existing single-run user-mode/CPL0 gate, and the strict comparator and manifest schema are unchanged. The focused 25-test series suite passes. At the time of the implementation commit, no live series had used the option; the following N=2 entry records its first use. Sidecar collection is diagnostic and is not an acceptance or normalization rule.
- **Current CPU0 ACPI sidecar N=2 (2026-11-16):** both unique-nonce runs loaded and reached the expected 3-billion budget stop; run 0001's nonce line merged with `Run /init as init process` and failed exact marker validation, while run 0002 passed. The series outcome was `failed`; strict comparison returned `matches=false` on scenario status, marker validation, machine observations, and normalized UART bytes. Complete schema-v5 sidecars captured 35 versus 33 delivered #PFs and 22 CPL0 RDTSC sites per run. The first 30 PF signatures match; run 0001 has two extra CPL0 user-stack write faults at `rep_movs_alternative+0x11` and `__put_user_8+0xd`. RDTSC differs by 17,759 reads, of which 17,614 are at the matched debug-ELF address `delay_tsc+0x4d`. These localize but do not explain either variation; no difference is classified benign and no comparator normalization changed. S5-07 remains blocked/unaccepted. [Evidence log](reports/data/stage5/s5-07-current-acpi-sidecars-n2-20261116.log).
- **Marker timing diagnostic (2026-11-16):** an isolated temporary initramfs moved the existing checked 1 ms sleep before the nonce write. In automatic N=2, both candidate-fixture runs emitted the exact nonce as a separate line after the kernel's `Run /init as init process` printk and reached the 2.6-billion budget stop. The strict comparator still found machine-observation and ordered-UART differences; delivered #PF counts were 34/33. The tracked guest fixture and parser are unchanged, and the timing hypothesis is supported only for the marker-interleaving symptom. S5-07 remains blocked/unaccepted. [Evidence log](reports/data/stage5/s5-07-marker-delay-before-nonce-n2-20261116.log).

- **Canonical fixture follow-up (2026-11-16):** the Stage 4 `/init` now places its existing checked 1 ms sleep before nonce output; the `getpid` check and strict comparator are unchanged. Rebuilt initramfs SHA-256 is `65acac38cd4c791154f33179fe2c9f2103dd7fdec51aacfd969ab4d9eb0aedb7`; builder suite: 23 tests, 22 passed, one skipped because staged kernel config is absent in this worktree. The candidate N=2 used the same ELF/CPIO bytes and validated both exact nonce lines, but comparison remains unexplained on machine observations/UART with 34/33 delivered #PF. This addresses the marker merge symptom only; ISSUE-081 stays open. [S4-36 follow-up](reports/STAGE_4_S4-36_2026-11-09.md); [N=2 evidence](reports/data/stage5/s5-07-marker-delay-before-nonce-n2-20261116.log).
- **Canonical fixture 3-billion N=2 (2026-11-16):** both unique-nonce runs on the updated initramfs passed exact marker validation and reached the expected budget stop. Strict comparison still returns `matches=false` for machine observations and ordered UART. Complete sidecars report 34 delivered #PFs and 22 CPL0 RDTSC sites in each run; the CR2 of startup stack event 30 differs, and total CPL0 RDTSC differs by 23,750 reads, mostly `delay_tsc+0x4d` (+23,649). These observations localize variation but do not explain it or establish benignity. S5-07 remains open/unaccepted. [Evidence](reports/data/stage5/s5-07-canonical-pre-marker-sleep-n2-20261116.log).
- **Canonical `norandmaps` sidecar N=2 (2026-11-16):** a fresh automatic diagnostic series with the canonical initramfs and opt-in `norandmaps` collected complete schema-v5 sidecars. Both unique-nonce runs passed marker validation and reached the 3-billion budget stop. Each recorded 33 delivered `#PF` events and 22 CPL0 RDTSC sites; ordered `(RIP, error_code, CR2)` fault signatures matched, so the event-30 CR2 difference from the no-`norandmaps` pair did not recur in this diagnostic. CPL0 RDTSC totals differ by 12,211, including 12,144 at `delay_tsc+0x4d`; UART lengths differ (23,513/23,893). Strict comparison still returns `matches=false` for machine observations and ordered UART. The diagnostic does not establish the cause of prior fault variation, explain timing variation, or prove benignity. S5-07 remains blocked/unaccepted. [Evidence and hashes](reports/data/stage5/s5-07-canonical-norandmaps-sidecar-n2-20261116.log).
- **Canonical `norandmaps` same-input controls (2026-11-16):** direct replays of both automatic-series runs with the same nonce and artifacts reproduced each source run's complete machine stop object, UART bytes and schema-v5 sidecar exactly; runner JSON differs only in sidecar output path. Thus each run's output is repeatable under its own guest-visible input, but the control does not isolate nonce causality, explain the cross-run differences, or establish benignity. The automatic strict comparator remains false and S5-07 remains blocked/unaccepted. [Evidence and hashes](reports/data/stage5/s5-07-canonical-norandmaps-same-input-replay-n2-20261116.log).
- **Canonical `norandmaps` UART semantic diff (2026-11-16):** after nonce and printk timestamp normalization, the ordered UART still has three differing regions: reported available memory (62,064/62,040 KiB), `sched_clock` calibration values, and the differing terminal panic unwind region (3 versus 11 optional frames, with `tty_ldisc_deref` in run 0002). The first raw mismatch is the `Zone ranges` timestamp, 1 μs apart. This localizes content differences but does not explain them or establish benignity; strict N=2 comparison remains false. [Diff method and hashes](reports/data/stage5/s5-07-canonical-norandmaps-uart-semantic-diff-20261116.log).
- **Same-input replay control (2026-11-16):** direct replays of both current canonical N=2 runs, each with its original nonce and identical kernel/initramfs/ACPI, budget, and CPU profile, exactly match the source run stop object, UART bytes, and schema-v5 sidecar; runner JSON differs only by sidecar output path. This establishes repeatability for the same guest-visible inputs. The inference that nonce-conditioned input accounts for the cross-run difference is not a proven mechanism; no observations are called benign, and strict acceptance remains open. [Evidence](reports/data/stage5/s5-07-canonical-same-input-replay-n2-20261116.log).

- **Kernel stack-offset diagnostic N=2 (2026-11-16):** with `norandmaps` and `randomize_kstack_offset=off`, both unique-nonce runs passed the scenario and exact marker checks and stopped at the 3-billion budget; each recorded 33 delivered `#PF`, with equal ordered signatures, and 19 CPL0 RDTSC sites without omissions or overflow. The CPL0 RDTSC spread was 6,798 reads (6,750 at `delay_tsc+0x4d`), down from 12,211 (12,144 at that site) in the preceding `norandmaps` pair. After nonce and printk timestamp normalization, only available-memory (62,020/62,048 KiB) and `sched_clock` regions differ; the 3-versus-11 terminal optional-frame difference did not recur. This is consistent with stack-offset randomization contributing to the unwind difference, but does not prove causality or benignity. Strict comparison remains false; S5-07 is still blocked/unaccepted. [Command, hashes and evidence](reports/data/stage5/s5-07-kstack-offset-off-norandmaps-n2-20261116.log).

- **Selected `delay_tsc` prefix trace N=2 (2026-11-16):** the opt-in sidecar captures post-RDTSC request/start/current/elapsed values and bounded context for one selected CPL0 RIP. A fresh unique-nonce pair retained 16,384 samples per run and explicitly counted the remaining 33,975,778 / 33,987,992 hits as omitted. Requested-cycle sequences and poll iteration counts match across the first 173 completed delay loops; elapsed values differ at the first poll (`0x669e`/`0x4f58`) despite the same `0x7d0` threshold and same exit decision. The prefix therefore confirms early elapsed-time variation but does not cover the later 99.95% of site reads or explain the full 12,214-read site-count difference. Overall strict comparison is still false for machine observations and ordered UART, with no comparator changes or benign classification. ISSUE-081 and S5-07 remain open; next step is a bounded distributed/later-window trace to find the first differing threshold decision. [Full trace evidence and hashes](reports/data/stage5/s5-07-rdtsc-delay-trace-n2-20261116.log).
- **Physical mini transfer/build follow-up (2026-11-16):** the committed tree, stage inputs and prior N=2 artifacts were transferred to the x86_64 mini; the recorded `series.json` and two observation-sidecar SHA-256 values matched. A user-level Rust 1.100.0 install enabled an offline release build of `od-runner`. A new N=2 attempt remained in its first 3-billion-instruction run for about an hour at 99.9% single-core use; its observation sidecar remained empty, the process was stopped, and the second run/comparator never ran. The owner notes that the mini CPU is old, but this attempt does not establish that as the cause of the duration. This is an incomplete diagnostic, not a scenario failure or acceptance result. No S5-07 status or comparator behavior changed. [Transfer and run record](reports/data/stage5/s5-07-mini-transfer-and-run-20261116.log).

<a id="issue-080-s5-06-guest-suite-unverified-and-f-endpoint-blocked"></a>



- **Same-run panic-context follow-up (2026-11-16):** counted run `OD5CTX20261115xxxxxxxxxxxxxxxxxxxxx` contains `? tty_ldisc_deref+0x16/0x20`. Its four sidecar captures sampled `io_serial_out` while printing the trace, so they do not establish whether the optional frame is stale or genuine. No allowlist/comparator change; further original unwind-context evidence is required. [Evidence log](reports/data/stage5/s5-07-panic-context-followup-20261116.log). S5-07 remains blocked/unaccepted.
- **Stride-512 `delay_tsc` trace and exact-input replays (2026-11-16):** a local macOS/aarch64 diagnostic N=2 on current CPU0 inputs passed both exact nonce markers and expected 3-billion budget stops, but strict comparison remained false for machine observations and nonce-normalized UART. Schema-v7 sidecars retained 66,384/66,387 samples across 33,988,924/33,990,416 selected-site hits, with zero omitted candidates. Across 66,384 shared ordinals, the derived `elapsed_cycles >= requested_cycles` outcome differed in 117 samples; the first differing sampled predicate was ordinal 1,255,424. Unsampled hits may differ earlier, and equal global ordinals do not align delay invocations. Manually removing printk timestamps still leaves available-memory (62,024/61,972 KiB) and `sched_clock` payload differences. Direct replays of each run with its same nonce and guest-visible inputs exactly reproduced its complete stop, UART, and full sidecar. The only recorded guest-visible input difference between source runs is the `od.nonce` value, so results are reproducible conditional on the recorded input; the mechanism is unknown and the differences are not classified as benign. The pair is `smoke_not_acceptance`/`diagnostic_not_counted`, not acceptance evidence; no comparator normalization or CPU behavior changed. [Detailed evidence and hashes](reports/data/stage5/s5-07-rdtsc-delay-stride512-n2-20261116.log). S5-07 remains blocked/unaccepted.

### ISSUE-080 — S5-06: guest suite проверен только частично на Linux; endpoint F отсутствует

- **Дата обнаружения:** 2026-11-12, при подготовке сценариев A–G.
- **Компонент:** `guest/linux/tests/`, `tests/fixtures/guest/stage5/`; проверка набора зависит от S5-01 и физического Linux стенда.
- **Владелец:** Stage 5 для скриптов и сценарных критериев; runner S5-07 для nonce/config/stop evidence; Stage 11 для модели NIC и сетевого endpoint.
- **Статус:** открыта; подготовленный скриптовый срез не принят. S5-06 остаётся `blocked` до обязательной Linux guest проверки и входов, перечисленных ниже.
- **Проверка доступности стенда (2026-11-15):** владелец явно разрешил передачу `guest/linux/tests/` и `tests/fixtures/guest/stage5/` в `/tmp`. Два bounded SSH подключения к `senya@192.168.1.14` завершились `Operation timed out` до аутентификации и удалённого выполнения; файлы не переданы, физических проверок этими попытками не было. Детали команд и результатов в [reachability log](reports/data/stage5/s5-06-stand-reachability-20261115.log). Повторить передачу, когда стенд/маршрут будет доступен.
- **Основание и последствия:** семь scripts A–G прошли host-side syntax check; пять guest C sources прошли Apple Clang syntax-only; nonce refusal проверен для всех семи. Read-only `debugfs` inspection of the accepted RAW confirmed `/usr/bin/gcc-15` is present as a symlink to the amd64 driver, while `/usr/bin/gcc` and `/usr/bin/cc` are absent. D/E/F/G scripts now name `/usr/bin/gcc-15` explicitly, matching the S5-05 C smoke and E static builder; a regression checks this path. This closes the known compiler-path mismatch only. Ограниченный Docker preflight 2026-11-13 (Docker 29.8.2, OrbStack aarch64; в `ubuntu:26.04` linux/amd64 образе gcc/gcc-15 отсутствовали) выполнен с `--network=none --read-only`, временными файловыми системами и read-only mounts. В `ubuntu:26.04` `bash -n` прошёл для семи scripts; B прошёл ephemeral write/verify; A и F отказались с ожидаемым exit 78 (`OD_S5_A_NOT_RUN` и отсутствие `S5_F_HOST`). Отдельный no-ELF preflight в нативном `linux/arm64` `python:3.11-slim-bookworm`, с сетью off, read-only root, tmpfs для `/tmp`, `/run` и `/var/lib`, и read-only mount всего `guest/linux/tests/`, прошёл `sh -n` для всех семи scripts. Там же A в safe default выдал `OD_S5_A_NOT_RUN` и завершился с 78 до мутаций; B прошёл write/verify с nonce и SHA-256 `4ec08e6729baed4d9c30884fc4078501b77ca12983c59329d1ca7bc412de059f`, выдав `OD_S5_B_PASS`. `guest_flush=not_sent`, поэтому результат B не подтверждает durability. Первоначально C и минимальное извлечение tar не прошли в эмулируемом `linux/amd64` образе `ubuntu:26.04` даже на tmpfs: GNU tar вернул `Function not implemented`; это ограничение данной эмулируемой среды, а не общий вывод о Docker backing filesystem. Повторный запуск минимального извлечения и самого `s5_c_archive_fs.sh` прошёл в нативном `linux/arm64` образе `python:3.11-slim-bookworm` под OrbStack aarch64, с `--network=none --read-only --tmpfs /var/tmp` и read-only mount скрипта (для воспроизведения: `docker run --platform linux/arm64 --rm --network=none --read-only --tmpfs /var/tmp -v "$PWD/guest/linux/tests/s5_c_archive_fs.sh:/suite/s5_c_archive_fs.sh:ro" python:3.11-slim-bookworm sh /suite/s5_c_archive_fs.sh s5c-preflight`). Все четыре source/extracted file hashes совпали; скрипт выдал `OD_S5_C_TREE_MATCH` и `OD_S5_C_PASS`. D/E/F/G guest C helpers прошли отдельную compile-only проверку; позднее D helper ELF прошёл отдельный изолированный runtime preflight, тогда как E/F/G ELF не запускались. Shell fail-closed gate F ранее завершился ожидаемым exit 78. Для compile-only прогона использовался кешированный `rust:1.99.0-bookworm` linux/amd64 (эмуляция на OrbStack aarch64), `--network=none --read-only --tmpfs /tmp:size=4g,exec`, RAW S5-05 и fixture directory mounted read-only. Перед и после прогона SHA-256 RAW `work/stage5/s5-21/stage5-rootfs.raw` совпадал (`c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895`); `debugfs rdump` извлёк систему в tmpfs, а `chroot` к `/usr/bin/gcc-15` сообщил версию `15.2.0-16ubuntu1`. С флагами `-std=c11 -O2 -Wall -Wextra -Werror -fno-ident -Wl,--build-id=none` успешно скомпилированы D из `stage5_d.c` + `stage5_d_math.c`, а также E/F/G из соответствующих одиночных источников. `file` подтвердил x86-64 dynamic PIE outputs; hashes всех пяти fixture sources совпали с fixture README. Compile-only контейнерный результат не подтверждает выполнение программ в Linux guest или на стенде. Отдельный D-only runtime preflight использовал кешированный amd64 image `rust:1.99.0-bookworm` (image ID `sha256:19059486ff9ddb4c6895cbc02a186f00274c257adcc6f79611ca29f55d36cdda`, эмулируемый на OrbStack aarch64) с `--network=none --read-only --memory=6g --cpus=2 --tmpfs /var/tmp:size=4g,exec`; accepted RAW, guest scripts и fixture directory были смонтированы read-only. RAW hash до/после остался `c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895`; через `debugfs rdump` содержимое было извлечено в tmpfs. Выполнялась только команда `S5_FIXTURE_DIR=/opt/open-desktop-stage5 chroot ... /bin/sh s5_d_compile.sh docker-d-only`. Fixture source hashes совпали с README; D output binary SHA-256 `ba79fd5111fb6191d985c9652b5588aae7fac001ac2a7b4bdffa552ba27897fc`, пустой `build.log` SHA-256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`; вывод: `OD_S5_D_RESULT nonce=docker-d-only sum=5050` и `OD_S5_D_PASS`. Следовательно D ELF был исполнен только внутри эмулируемого Linux container; это не booted Linux guest, не runner integration и не физическая acceptance. E/F/G helpers только compile-only, их ELF не исполнялись; durability не проверялась. Эти проверки — только container shell/изоляционный preflight; образы не являются Linux guest стенда, поэтому ни один сценарий не принят по результатам Docker и обязательная проверка на физическом стенде остаётся. Отдельный host-only F classifier harness проверяет connect-error allowlist и drop outcomes без сокетов. Сценарий A безопасно отказывается от poweroff без предварительных test-only kernel cmdline opt-in, nonce match и VM indication; текущий runner не доставляет эти поля и не подтверждает `MachineStopReason::Shutdown`. Сценарий F требует удалённый TCP echo, accept-and-close и разрешаемый, но недоступный endpoint; сервис не реализован, а `S11-32`, `S11-31e` и `S11-33` не приняты. Локальный файл или loopback не являются заменой сетевого пути. Из-за `S5-01` и отсутствия полного Linux guest/scenario run нельзя утверждать работу сценариев в госте, JIT/SMC или persistence.
- **Обновление (2026-11-14, физический Linux preflight):** после разрешённой передачи на mini (Ubuntu 26.04.1 LTS, Linux 7.0.0-42-generic, x86_64; GCC-15 15.2.0-16ubuntu1) SHA-256 15 файлов совпал с рабочим деревом. Все scripts A–G прошли sh -n и invalid-nonce fail-closed; A безопасно вернул NOT_RUN без poweroff; C/D/E завершили host-side checks; F отказал без endpoint; G отказал до persistent writes на лимите 129 MiB; guest C sources прошли syntax-only, F host classifier прошёл без сокета. Это только physical host preflight: B positive path, A shutdown, G stop, F network и guest execution не проверялись. S5-06 остаётся blocked/unaccepted. Компактное evidence: [S5-06 physical Linux preflight](reports/data/stage5/s5-06-physical-linux-preflight-20261114.log).
- **Обновление (2026-11-16, mini доступен по новому адресу):** SSH к `senya@192.168.1.39` подтвердил тот же `mini`, Ubuntu x86_64 / Linux `7.0.0-42-generic`, GCC-15 15.2.0-16ubuntu1. Разрешённые `guest/linux/tests/` и `tests/fixtures/guest/stage5/` переданы во временный `/tmp`; хеши 15/15 совпали. Повторный ограниченный host-side preflight прошёл: syntax и nonce refusal A–G, C/D/E, safe A default, F без endpoint, G cap rejection и syntax-only шести C fixtures. Сценарные ограничения остаются: нет guest boot, B positive durability, A poweroff, G stop orchestration или сетевого F transfer. Полные результаты: [mini address preflight](reports/data/stage5/s5-06-mini-address-preflight-20261116.log).
- **Следующий шаг:** после входов S5-01 и доступного стенда выполнить все семь scripts внутри обычного Linux guest; runner S5-07 должен передать nonce и A-опт-ин через конфигурацию boot командной строки, затем записать observed shutdown reason. Согласовать F endpoint с S11-33 и провести transfer/drop/unreachable checks только через фактический NIC/NAT путь. До этих проверок оставлять S5-06 unaccepted; E ограничен interpreter profile, JIT claim ждёт S3-23/S3-24.

<a id="issue-079-s5-21-accepted-profile-registry-is-incomplete"></a>

### ISSUE-079 — S5-21: профильный реестр ISA не является полным CPL0/CPL3 allowlist

- **Дата обнаружения:** 2026-11-12, при статическом анализе ELF из принятого S5-05 rootfs.
- **Компонент:** S5-21 ISA inventory/profile comparison; вход для этапов 2 и 4.
- **Владелец:** Stage 2/4 — определение и приёмка instruction profiles; Stage 5 сохраняет измеренные guest-binary frequencies.
- **Статус:** открыта; S5-21 в `blocked` и не принята. Inventory/handoff и source-backed taxonomy expansion приняты как reviewable static-evidence slices; требуется закончить disposition всех наблюдаемых форм относительно принятых Stage 2/4 профилей и gates. Реестр остаётся `registry_complete=false`. Это статический inventory blocker; runtime-generated E/JIT/SMC evidence и S3-23/S3-24 JIT gates относятся к сценарию S5-12 и не являются частью S5-21.
- **Основание и последствия:** последняя source-backed inventory использует восемь ELF из принятого rootfs SHA-256 `c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895`; E-inclusive input добавляет статический ELF SHA-256 `da18de76182af4f986a076c8ec9796ae0db2af365104788929e22643143d8024`. Последняя rootfs analyzer pair идентична на raw SHA-256 `bbd593c567fda85f4b95c062509aa7990b34da5fdf4cb6922065aac6cce2bddc`, prior compact gzip SHA-256 `1ca3bf6e0fb6f016cc9828bc0c31942e158dec9f5dded413b0fbd4ede9c4bab6`; current compact annotation SHA-256 `7dceca57748810beb6d1c05802c9d1a83aeb6e2c61faa0d36d3eab6b1864b3a9`; E-inclusive pair — raw `1ac9f29ab66560bac0c0ece157308e7647585e18c8201988f3986b3a218e9570`, prior compact gzip `7d6e0f93349a01614b50aeb52ae94a6611f5c3ffb4bbf4e2e812d39d9c53c426`; current compact annotation `a4032b62e963025f4e0020306968807cd306e91e56ba15a9b285a51b08cb47ad`. Rootfs содержит 6 510 400 instruction rows (6 296 422 bounded-profile evidence, 213 978 unclassified); E-inclusive — 6 510 745 rows (6 296 706 evidence, 214 039 unclassified). В обоих наборах 1 064 из 1 269 normalized forms имеют bounded evidence, 205 остаётся unclassified. Принятые local-only профили S2-35.35, S2-35.47, S2-35.41, S2-35.45, S2-35.39, S2-35.42, S2-35.43, S2-35.7, S2-35.2, S2-35.6, S2-34.9a, S2-34.9c, S2-34.8c, S2-35.21, S2-35.18, S2-35.17, S2-34.13 и S2-35.33 добавляют 805 PSHUFD XMM register-register rows / 2 формы, 355 PSRLDQ register-immediate rows / 2 формы, 2 PSLLD rows / одну форму, 5 PSRLQ rows / одну форму, 6 PSRAW rows / 2 формы, 11 PSRLD rows / одну форму, 12 PSRAD rows / одну форму, 1 PSUBW row / одну форму и 2 PADDB XMM register-register rows / одну форму (2 exact encodings) плюс 2 PADDW rows / одну форму (1 exact encoding) и 2 MOVSD register rows / одну форму (1 exact encoding), а также 3 PCMPGTB rows / 2 формы / 3 exact encodings и 6 PCMPEQB rows / одну форму / 2 exact encodings и 12 PSUBQ rows / одну форму / 5 exact encodings и 14 MOVAPD forward register rows / одну форму / 7 exact encodings и 16 PACKUSWB register rows / 2 формы / 7 exact encodings и 40 PCMPGTD register rows / одну форму / 18 exact encodings, 98 MOVAPS forward register rows / одну форму / 12 exact encodings и 126 PCMPEQD rows / две формы / 18 exact encodings и 7 PMOVMSKB rows / одну форму / 3 exact encodings и 8 PMULUDQ rows / одну форму / 2 exact encodings и 77 PADDQ rows / одну форму / 8 exact encodings и 132 PADDD rows / одну форму / 21 exact encoding; E ELF новых строк этих инструкций не добавляет. CPU_CONTRACT не содержит PACKUSWB entry; учтена только принятая bounded S2 plan/report область. CPU_CONTRACT не содержит PSUBQ entry; учтена только принятая bounded S2 plan/report область. PCMPEQB observed as `66 0F 74 C3` ×3 and `66 0F 74 CC` ×3; equal byte lanes become FF, unequal lanes 00. PCMPGTD adds 40 rows in 18 exact encodings, comparing four signed i32 lanes DEST>SRC and writing FFFFFFFF or zero; CPU_CONTRACT has no entry, so only accepted plan/report evidence is counted. MOVAPS S2-34.11 adds 98 forward register rows in 12 encodings and copies raw 128-bit values; alignment/fault semantics are not claimed. PCMPEQD adds 126 rows in 18 exact encodings, comparing four u32 lanes for whole-dword equality; CPU_CONTRACT has no entry, so only accepted plan/report evidence is counted. PCMPGTB observed as `66 0F 64 C8`, `66 44 0F 64 C0` and `66 0F 64 D8`; signed i8 lanes write `FF` iff destination>source else `00`. PSUBW observed once as `66 0F F9 C1`, with ModR/M.mod=3, destination XMM0, source XMM1. PADDB observed as `66 0F FC C8` and `66 0F FC D0`, with ModR/M.mod=3; each byte lane adds modulo 256. PADDW observed twice as `66 0F FD C1`, with ModR/M.mod=3; eight little-endian u16 lanes add modulo 65536. MOVSD register form observed twice as `F2 0F 10 CA`, copying the raw source low qword while preserving the destination high qword. MOVSD S2-34.9c adds three exact `F2 0F 10 00` `[RAX]` m64 load rows (gcc-driver, gcc-cc1, gcc-collect2 one each); within the existing 88-row base-only/F2 normalized form the disposition is mixed, with 3 supported and 85 unclassified. The 36 RIP-relative and five SIB MOVSD memory rows remain unclassified. Both x87 and SSE state, SSE2/CR0/CR4 assumptions, a successful canonical48 span check and one 8-byte read are prerequisites to the bounded local profile. Static rows do not infer runtime RAX values, mapped data or guest read success; failed fetch/read/range commits no CPU state, and no guest fault-order or CPL3/native/JIT/physical claim is made. The accepted S2-34.9c independent re-review is recorded at Stage 2 report 14481–14484; focused inventory review passed four ISA checks (three load_rax plus decoder), one schema test and one fixture. All 16×16 register-pair combinations and optional REX.R/B register-extension combinations are covered by focused matcher tests. PSRAD представлен 12 строками только с `66`, `/4`, ModR/M.mod=3; unsigned imm8 counts >=32 sign-fill отрицательные i32 lanes и очищают неотрицательные. PSRLD observed one `66` form, `/2`; its counts >=32 clear dword lanes. PSRAW представлен пятью строками с `66` и одной с `66 41`. PSLLD представлен формой только с `66`; PSRLQ встретился 4 раза с ModR/M `D0` и 1 раз с `D3`, все с `66`. PSRLDQ распределяется как 345 строк с `66` и 10 строк с `66 41`; no PSLLDQ rows were observed. Одна PSHUFD memory row остаётся unclassified. Историческая сводка до свежего PAND review: шесть local-only профилей S2-34.7a–.7d и S2-35.50/.51 классифицировали 1 858 register-register rows / 8 forms (XORPS 1 261, ANDPS 1, ORPS 21, ANDNPS 4, PANDN 133, POR 438); на тот момент PAND ожидал review. Теперь PAND S2-35.49 принят для bounded local-only accounting: итоговая группа содержит 2 162 register-register rows / 10 forms. Всего 179 SIMD logic memory rows остаются unclassified, включая 52 PAND memory rows. Ни один target ELF не исполнялся; эти local profiles не устанавливают CPL3/native/JIT поддержку. PADDD S2-35.14 adds 132 rows in one form and 21 exact encodings (gcc-cc1 109, tar 18, gcc-as 3, gcc-ld.bfd 2); four independent u32 lanes add modulo 2^32. Both x87 and SSE state are required. Fresh review passed 3 ISA checks, 1 schema test and 1 fixture; Stage 2 report has no PADDD-specific entry. The native guard is early; its Linux assertion was compiled but not run on this Mac. Fresh PAND-only review (3 ISA, 2 testkit, 1 fixture) is new bounded evidence and does not revise the historical pending-review record.
- **Сохранённые evidence:** [S5-21 report](reports/STAGE_5_REPORT.md#14-статус-s5-21-2026-11-12), [2026-11-13 conditional-local follow-up](reports/STAGE_5_REPORT.md#s5-21-conditional-local-conversion-follow-up-2026-11-13), [compact inventory](reports/data/stage5/s5-21-isa-inventory-e-static.json.gz), [raw inventory в ignored work](../work/stage5/s5-21/s5-21-isa-inventory-scalar-arith-e-pass1.json), [local profile registry](../tools/build/stage5_isa_profile_forms.json).
- **CPL0 уточнение:** источник S4-38L7x принимает byte PIO в `linux_default`, поэтому старое исключение `EC` удалено; в данных L7x не встретилось. Candidate taxonomy и tests теперь охватывают accepted S4-38L7x–as без вывода CPL3 поддержки. Из observed L7 candidates есть L7y LOCK CMPXCHG dword — 9 rows (C 0 / D 9) и L7z LOCK ADD memory immediate — 33 (C 0 / D 33); прочие новые L7x/aa–as формы в этих ELF не встретились. WRMSR `0F 30` не содержит ECX index; L7ao–as остаются объединённым `WRMSR-index-unobserved` candidate и не считаются index-specific evidence. Ни один target ELF не исполнялся.
- **Обновление S4-38L9p (2026-11-16):** после merge accepted bounded CPL0 profile включён в registry; compact-only annotation покрывает 13 точных unprefixed `87 /r` dword-memory rows только scenario D (gcc-cc1 5, gcc-collect2 4, gcc-driver 4), одна normalized form/три exact keys. Rootfs/E compact SHA-256: `3e0d006622e9f21af3c54048c145e6ed21d30bd29ae6225ae7c96e0eff6ba913` / `52e9c9672ccd4930540e7e4feb2147f4d1dabfa41a7d1fa03377a067129144f2`. Candidate labels не меняют classification, evidence scope или profile matches; все строки остаются unclassified. Raw analyzer не перезапускался, ELF не исполнялись, а S4 CPU contract по-прежнему помечает SDM source как unchecked. Остальные profile dispositions открыты; S5-21 остаётся blocked/unaccepted. Подробности и точные gates — в [S5-21 отчёте](reports/STAGE_5_REPORT.md#s5-21-s4-38l9p-conditional-cpl0-candidate-2026-11-16).
- **Следующий шаг:** Stage 2/4 должны дать source-backed disposition и explicit execution gates для оставшихся 205 normalized forms / 213 978 rootfs-only rows (214 039 с E ELF) относительно bounded CPL0/CPL3 profiles, чтобы получить точный missing-form count; универсальный OS allowlist не требуется. S5-21 остаётся в границах статического сравнения: S5-06 нужен только как источник собранного E static ELF. Эта запись не требует исполнения ELF и не отслеживает runtime-generated E/JIT/SMC bytes; их сценарный blocker ведётся в S5-12 с отдельными S3-23/S3-24 gates. PAND S2-35.49 now has fresh bounded local-only evidence; 179 SIMD memory-logic rows остаются unclassified из-за отсутствия подходящего принятого профиля, включая 52 PAND memory rows. PMOVMSKB S2-35.61 classifies seven `gcc-cc1` rows in one form and three exact encodings (`66 0F D7 C0` ×3, `C9` ×3, `C8` ×1); low 16 bits capture source byte sign bits and the GPR32 write zero-extends. Both x87 and SSE state are required, with atomic refusal when either is absent. This is a fresh review for the bounded inventory slice only; no CPL3/native/JIT evidence is implied. PMULUDQ S2-35.62 adds eight `gcc-cc1` rows in one form and two exact encodings (`66 0F F4 C3` ×4 and `66 0F F4 E9` ×4); it multiplies unsigned source lanes 0 and 2 into two destination qword lanes and ignores odd source lanes. Both x87 and SSE state are required, with atomic refusal when either is absent. This is fresh review evidence for this slice only. MOVSS S2-34.8a adds 15 rows in one form and five encodings (`F3 0F 10 CA` ×7, `C1` ×3, `D0` ×3, `C5` ×1, `C6` ×1), distributed across bash, gcc-cc1, gcc-collect2, and gcc-driver. It copies raw source low32 to destination low32 and preserves destination upper96; no floating-point, MXCSR, or flags effect is claimed. Both x87 and SSE state are required. This is new fresh-review evidence for this bounded slice only. PSUBD S2-35.15 adds 24 gcc-cc1 rows in one form/eight exact ModR/M encodings (`C8`×7, `F5`×6, `C1`×5, `F4`×2, `CD/D5/DD/EA`×1); four independent little-endian u32 lanes subtract modulo 2^32 with no inter-lane borrow. Both x87 and SSE state are required; SSE2/CR0/CR4 are assumptions. The fresh review passed three ISA tests, one schema test and one fixture. S2-37i MINSD adds four local state-gated candidate rows in one normalized form/two exact encodings (`F2 0F 5D C8` ×2 and `CA` ×2), all gcc-cc1; they remain unclassified and receive no evidence-scope or supported credit. The MXCSR predicate is `(MXCSR & 0xffff9fc0)==0x1f80`; runtime state is unmeasured. Both x87 and SSE interpreter state are required. S2-35.16 PADDQ adds 77 rows in one form/eight encodings (`C1`×48, `C8`×11, `C2/C3`×6 each, `CA/D4`×2 each, `C5/D1`×1 each), distributed across gcc-cc1 (63), gcc-collect2 (5), gcc-driver (5), gcc-ld.bfd (1), and tar (3). Two little-endian u64 lanes add modulo 2^64 independently with no inter-lane carry. Both x87 and SSE state are required; SSE2/CR0/CR4 are assumptions. CPU_CONTRACT has no PADDQ entry, so only plan/report evidence is credited. Fresh review passed three ISA checks and one fixture; there is no od-testkit PADDQ schema test (0 tests). The native runner has an early local-only guard; its Linux-gated assertion compiled but was not run on this Mac. Native/JIT/physical acceptance is not claimed. Runtime-generated E bytes остаются вне static S5-21 inventory; их frequency нельзя выводить из этих ELF. Этот runtime/JIT/SMC сценарий отслеживается отдельно в S5-12 после собственных зависимостей и S3-23/S3-24 gates; он не является blocker для статической задачи S5-21. Inventory сама по себе не является guest pass/fail. PADDD S2-35.14 adds 132 register-register rows in one form/21 encodings; E adds none. It adds only local interpreter evidence, with x87+SSE state required and no native/JIT acceptance. The PAND review is a new slice review; the prior Stage 2 plan entry remains historical and is superseded by its dated follow-up. MOVSS S2-34.8c adds three tar rows in two exact encodings (`F3 0F 10 10` ×2 and `F3 0F 10 08` ×1); the existing 14-row base-only/F3 normalized form is mixed (3 supported, 11 unclassified). Both x87 and SSE state are required, with SSE/CR0/CR4 assumptions only and no SSE2 claim. A successful canonical48 `[RAX..RAX+3]` check and one 4-byte read precede low32 write, upper96 clear and RIP update; static evidence does not establish runtime RAX/data/read success. This fresh S2-34.8c review passed two ISA checks (executor and decoder), one schema test and one fixture; no CPL3/native/JIT/physical or guest-fault claim is implied. MINSD S2-37i is additionally tracked as four local state-gated conditional candidates (one form/two encodings, gcc-cc1); all four remain unclassified and do not change the 205-form residual. The predicate requires SSE present, both raw x87/SSE interpreter state, `(MXCSR & 0xffff9fc0)==0x1f80`; runtime MXCSR is unmeasured, with no default inferred. Generic conditional totals include these four rows; CPL0-specific counters and the 40-row supported overlap are unchanged. S2-37q/r/v/w are additional state-gated local candidates: q168 (3 forms/50 keys), r50 (3/23), COMISS10 (1/8), COMISD24 (1/6), UCOMISD10 (1/6), and UCOMISS0; all 262 rows remain unclassified. q/r/v/w require `(MXCSR & 0xffff0040)==0`, SSE payload state, and canonical profile bytes; schema x87-payload exclusion is fixture-format-only. The fresh q/r review follow-up (2026-11-13) is in the Stage 2 plan. Before the S2-37be annotation, across 507 local state-gated candidate rows, including 186 scalar arithmetic rows from S2-37ap/at/au/ax/ay/bb/bc (8 normalized forms /72 exact keys), generic conditional hits/unique rows are rootfs113739/113662 and E-inclusive113799/113722; CPL0-specific counts remain 113232/113292 and supported overlap40 is unchanged. All arithmetic candidates require the SSE payload, `(mxcsr & ~0x0000ffbf)==0` and OM=UM=1; RC, FTZ, exception masks other than OM/UM, and sticky flags are not inferred. Fresh DIVSS semantic review at HEAD 4ffd54f confirms conditional-only metadata eligibility for its 13 rows, not full unmasked OE/UE or physical acceptance. New S2-37t/ad/ae add 55 unclassified rows: t 19, ad 8, ae 28, distributed gcc-cc1 39, tar 12 and dash 4 in both datasets. These comprise 6 normalized instruction-form keys (5 operand signatures) and 24 exact keys; CVTTSD2SI r32 splits into F2/no-REX 10 rows and F2+REX 8 rows. All remain outside evidence_scope/proven_supported.

- **CVTSD2SS conditional-local annotation (2026-11-13):** the existing compact exact-form records contain exactly one `F2 0F 5A F1` row (`cvtsd2ss xmm6,xmm1`) in `gcc-cc1`; it remains unclassified/outside `evidence_scope` and `proven_supported`. The local candidate total is 508 (rootfs and E-inclusive); rootfs/E generic conditional hits/unique rows are 113740/113663 and 113800/113723. Supported/unclassified totals, 205 open forms, CPL0 counts and 40-row overlap are unchanged. This is a guarded annotation of the existing compact row; raw analyzer pass hashes remain the previous verified pair and no new bytewise raw rerun is claimed. Candidate gate, MXCSR/control assumptions, raw x87 exclusion, limited semantic review and artifact hashes are recorded in the [Stage 5 report](reports/STAGE_5_REPORT.md#s5-21-conditional-local-cvtsd2ss-follow-up-2026-11-13) and [manifests](reports/data/stage5/s5-21-repeatability.json).


- **Обновление (S5-21 CPL0 candidate annotation, 2026-11-13):** compact-only derivation добавила PAUSE `F3 90` (6 rows) и bare LEAVE `C9` (2 005 rows); обе остаются unclassified/outside evidence_scope/proven_supported. Rootfs/E CPL0 candidate hits 115243/115303; generic hits 115751/115811; unique rows 115674/115734; overlap40, local rows508 и classification totals без изменений. Raw analyzer hashes сохранены; no new bytewise raw rerun is claimed. Reproducible compact hashes: rootfs `b9059f45adda311f3a11950c440d75e67c22d255c7d2899d13b76c7dc3d12377`, E `d266793df5c0f924cd77f76b57f9c7898bca721be5c41989de8395ed5039084c`. LEAVE's Stage 4 Intel SDM source attribution remains unverified; see [ISSUE-083](#issue-083-stage4-leave-sdm-source-unverified) and [S5-21 report](reports/STAGE_5_REPORT.md#s5-21-cpl0-conditional-candidate-follow-up-pause--leave-2026-11-13).

S5-21 follow-up (2026-11-13): compact-only conditional annotation now includes 398 observed REP-BSF register rows (72 exact keys), requiring Long64, BMI1=0 and composed CPL0 gate; two observed memory rows remain excluded. All candidate rows remain unclassified/outside evidence_scope. Current compact hashes are rootfs `7dceca57748810beb6d1c05802c9d1a83aeb6e2c61faa0d36d3eab6b1864b3a9` and E-inclusive `a4032b62e963025f4e0020306968807cd306e91e56ba15a9b285a51b08cb47ad`; raw analyzer hashes are unchanged, raw analyzer was not rerun, and no target ELF was executed. The exact derivation and candidate-only semantic review are recorded in the [S5-21 report](reports/STAGE_5_REPORT.md#s5-21-cpl0-conditional-candidate-follow-up-rep-bsf-registers-2026-11-13).

S5-21 follow-up (2026-11-13, S4-38L7ay): a deterministic compact-only overlay adds 36 exact `LOCK XADD m32,r32` rows (one normalized form/six exact keys; gcc-cc1, gcc-collect2 and gcc-driver 12 each). The matcher is limited to `F0 0F C1 /r`, ModR/M.mod=01 + disp8, length 5, 64-bit base-only addressing without REX/SIB/segment; the candidate also requires Long64, Ring0 and composed CPL0 `control_policy`. Rows stay unclassified/outside evidence scope. The accepted memory model reads source/address from pre-instruction state and performs the required dword read/write (including zero source) on one vCPU; no multi-vCPU atomicity or runtime mapping/permission/backend claim is made. Rootfs/E CPL0 hits are 115683/115743, generic hits 116191/116251, unique rows 116114/116174; classification totals, overlap40, local508 and 205 residual forms are unchanged. Compact hashes are rootfs `5b688b74a78417bab072e2e4c3f749cb187feb7aa6fc259f3ae07edb118079f2` and E `40dae558902d1713b6aef259cf99d7646957c13a1c7e8ca3e38c4012a727c239`; historical raw hashes remain and no analyzer rerun or target ELF execution is claimed. Scenario C/D candidate maps and row totals are now regenerated from exact forms and their binary membership (21,176/111,669 rows); XADD contributes 0/36, and local candidate maps remain separate. The Stage 4 contract's historical SDM attribution remains marked unchecked; a separate candidate review verified Intel SDM 325383-093US, Vol. 2D, pp. 6-27–6-28 (PDF SHA-256 `d137a7887787bbb06c1a8b8bafd51958d8d2e16b4dff987a03226c845cca1516`, [official PDF](https://cdrdv2-public.intel.com/929352/325383-093-sdm-vol-2abcd.pdf)). See the [S5-21 report](reports/STAGE_5_REPORT.md#s5-21-cpl0-conditional-candidate-follow-up-lock-xadd-dword-memory-2026-11-13). S5-21 remains open and `registry_complete=false`.

S5-21 residual handoff (2026-11-13): both rootfs and E-inclusive exact inventories contain the same 292 `3E FF /4` register forms: `3E FF E0` ×252, `3E FF E2` ×30, `3E FF E1` ×8, `3E FF E6` ×2. All remain unclassified/outside evidence scope; exact records contain no REX.B and no REX prefix. Current S2-16d/e acceptance covers ordinary `FF /4` register forms with REX.B and ordinary memory forms, but not `3E`; the Stage 5 registry has no NOTRACK entry and the decoder rejects segment-prefixed register-ModRM indirect JMP. The prefix's IBT effect depends on CET runtime state (`IA32_U_CET.NO_TRACK_EN` or `IA32_S_CET.NO_TRACK_EN`), so static bytes do not prove tracking suppression. Next owner: Stage 2 exact prefix semantics, CET/IBT gates and fault/no-commit boundaries; Stage 4 may separately select a CPL0 gate; Stage 5 annotates only after acceptance. No code or inventory was changed. See the [residual report](reports/STAGE_5_REPORT.md#s5-21-residual-handoff-notrack-register-jmp-encodings-2026-11-13).

**Строгий ranking остатка (2026-11-13):** прежние 205 broadly unclassified normalized forms, 213 978 rootfs rows и 214 039 E-inclusive rows — широкие classification-snapshot totals, включающие строки с conditional candidates; они не являются количеством форм без candidate/evidence/profile annotations. Строгий ranking включает только normalized forms, у которых aggregate и каждый exact key имеют исключительно `unclassified` и пустые conditional-candidate, conditional-local-candidate, evidence-scope и profile-match maps. Частично совпавший exact key исключает всю normalized form. Строгий остаток: rootfs 97 808 rows / 157 forms / 1 973 exact keys; E-inclusive 97 809 / 157 / 1 973; scenario C 2 213 / 61 / 322; scenario D 97 152 / 149 / 1 937. E delta: один rootfs-to-E row, ноль новых forms/keys; для C/D delta нулевой. Все эти rows по-прежнему unclassified; ranking не переименовывает их в unsupported и не меняет CPL0 candidate totals. Сохранены CPL0 hits rootfs/E 115 683/115 743, generic hits 116 191/116 251 и unique rows 116 114/116 174. Подробности: [ranked Markdown](reports/data/stage5/s5-21-ranked-residuals.md), [machine-readable JSON](reports/data/stage5/s5-21-ranked-residuals.json), [Stage 5 report](reports/STAGE_5_REPORT.md#s5-21-ranked-strict-residual-handoff-2026-11-13). S5-21 remains blocked and `registry_complete=false`.

- **Внешний условный handoff (2026-11-13, main S4-38L9p):** read-only comparison against main HEAD `2f45e60` found accepted S4-38L9p docs `82695c5`, implementation `6ce2e01` and contract `9742536`. Their exact intersection is 13 scenario-D residual rows, one normalized form, three exact keys: `87 07` ×6, `87 43 disp8` ×4, `87 47 disp8` ×3; scenario C and E have no delta. Current codex implementation has qword-memory XCHG only; these dword forms are absent. Main `CPU_CONTRACT` says its SDM source was not independently checked. This is a conditional external handoff only: require a branch integration decision and source-backed semantic review before any credit. Do not alter candidate maps, support/evidence classification, counters or ranked residual artifact. L9m/L9n have no matches; L9o has no exact bare fence rows (`0F AE` observations are prefixed INCSSPQ, not fences). S5-21 remains blocked/unaccepted.

<a id="issue-078-s5-05-rootfs-extraction-toolchain-blocked"></a>

### ISSUE-078 — S5-05: extraction toolchain failure on emulated builder

- **Дата обнаружения:** 2026-11-11, при проверке сборки Stage 5.
- **Компонент:** rootfs extraction/import и deterministic filesystem build.
- **Статус:** решена; сборочный срез S5-05 принят после независимого ревью рецептов и артефактов.
- **Исходное основание:** официальный Ubuntu Base 26.04.1 amd64 archive `ubuntu-base-26.04.1-base-amd64.tar.gz`, SHA256 `a496a960472ce474a59590b8987d3a1135d3cbef1991f3b1abe8cacfea8bf85a`, was verified against its signed `SHA256SUMS` manifest. Extraction under an emulated amd64 container failed with GNU tar `Function not implemented`; evidence remains in [the original extraction log](reports/data/stage5/s5-05-container-extract.log). The host `mke2fs 1.47.4` was built without libarchive and the synthetic tar import failed; evidence remains in [the tar import probe](reports/data/stage5/s5-05-mke2fs-tar-probe.log). Those failures were specific to the initial toolchain path.
- **Verified resolution:** the same signed Ubuntu Base archive and hash-locked package overlay extract successfully using pinned Ubuntu ARM64 builder `ubuntu@sha256:86eb1bc1059720a19a777ba5a109e9ee3690d5dfe3e319871a6d6beb86fb7f85` (local image ID `sha256:490aa88f39f72f944cc111695f97fa6f78f4f3cc047a333f9448e6f03d4089b1`, `linux/arm64`). e2fsprogs 1.47.2 normalizes ext4 metadata deterministically; `e2fsck -fn` passed for both fresh builds. The pinned kernel packages are version 7.0.0-38.38; config confirms NVMe modular and ext4 built in. The initramfs contains `hkdf`, `nvme-auth`, `nvme-keyring`, `nvme-core`, and `nvme`, generated from the kmod dependency output; cpio listing contains no host-path/secret names.
- **Results and reproduction:** rootfs paired test `python3 tools/build/test_build_stage5_rootfs.py` passed on 2026-11-11 (`Ran 1 test in 16.542s; OK`), both image SHA-256 values `c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895`; the C smoke compiles, links, and executes (`stage5-c-smoke-ok`), and both ext4 filesystems pass `e2fsck -fn`. Separate initramfs paired test `python3 tools/build/test_build_stage5_initramfs.py` passed (`Ran 1 test in 4.626s; OK`), both archive SHA-256 values `17621413fae2456b911fa99d2949daeca8cc13801c5d5a4951b85030f8d8f4b1`, kernel SHA-256 `9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e`; cpio listing has five modules and `/init` moves `/proc`, `/sys`, `/dev` and console. Both tests inject an invalid unlocked `.deb` between builds and retain identical hashes. Full output: [rootfs paired log](reports/data/stage5/s5-05-rootfs-pair-review.log) and [initramfs paired log](reports/data/stage5/s5-05-initramfs-pair-review.log). Package locks, provenance, commands, tool versions, root device and S4-35 caveat: [Stage 5 fixture README](../tests/fixtures/guest/stage5/README.md). Accepted safe RAW-creation prerequisite remains commit `e62b6c1`; it uses the shared Rust creation policy/helper and does not create the filesystem.
- **Remaining boundary:** no Linux guest boot or disk scenario was run. This issue is closed for artifact construction. S5-05 acceptance does not claim guest NVMe discovery, PCI binding, IRQ delivery, a Linux guest boot, or acceptance of S4-35's earlier artifact provenance.

<a id="issue-077-s5-03-raw-quarantine-not-persistent-across-process-exit"></a>

### ISSUE-077 — S5-03: quarantine RAW не переживает завершение host-процесса

- **Дата обнаружения:** 2026-11-11, при реализации и проверке S5-03.
- **Компонент:** `crates/od-host/src/persistent_nvme.rs`, lifecycle persistent RAW backend.
- **Владелец:** Stage 5 host integration; inspection/reconcile procedure принадлежит Stage 9.
- **Статус:** открыта; текущий adapter хранит terminal/quarantine state только в памяти процесса. On-disk sidecar или durable quarantine registry не задан и не добавлялся.
- **Основание и последствия:** runtime write/flush failure переводит adapter в terminal state и удерживает `HostFileStorage` lock до закрытия adapter; `CheckedStorage::ReconcileRequired` не сбрасывается обычным переоборачиванием. SIGKILL в окне после фактического host write и до durable acknowledgement освобождает OS `flock`, а процессная quarantine state исчезает. Следующий процесс может открыть тот же RAW path без проверки, что неизвестные bytes inspected/reconciled. В S5-17 повторно проверен изолированный process-kill test: он подтверждает отсутствие успешного backend return до kill, но не читает образ после kill и не делает утверждений о состоянии/долговечности bytes.
- **Следующий шаг:** до разрешения повторного использования после process death определить согласованный inspection/reconcile и host lifecycle gate. Не добавлять sidecar marker, пока не задана и не испытана его атомарность, cleanup и поведение при crash.

<a id="issue-076-s5-01-run-duration-inputs-missing"></a>

### ISSUE-076 — S5-01: неизвестны inputs для повторяемой серии и soak duration

- **Дата обнаружения:** 2026-11-11, при подготовке методики S5-01.
- **Компонент:** S5-01 Method/Numeric gates, критерии S5-15/S5-16; входы `S4-39`, принятого S5-08 boot path и принятого C+D+G cycle S5-10/11/14.
- **Владелец:** Stage 5; входы Linux userspace зависят от Stage 4/S4-39, workload — от сценариев этапа 5.
- **Статус:** открыта; Method gate принят как отдельный subgate после независимого ревью. Numeric calibration gate blocked, поэтому полный S5-01 остаётся blocked; числовые `N` и `D_soak_estimate` не определены и не являются acceptance evidence.
- **Основание и последствия:** [S4-13](plans/STAGE_4.md#s4-13-выбрать-простой-программный-tlb-по-замерам) измерил 200,000,000 retired instructions за 554 s user CPU (runner evidence: [s4-13-fetch-cache-200m-run.json](reports/data/stage4/s4-13-fetch-cache-200m-run.json)), то есть `R = 200,000,000 / 554 = 361,010.8303` retired instructions/user-CPU-s. Владелец подтвердил, что этот прогон выполнен на «этом PC»; текущая проверка окружения идентифицирует PC как Mac15,11/arm64 под macOS 26.7.1 build 25G241. Сохранённые JSON и строка отчёта не фиксируют host/OS/build на момент измерения: host attribution основана на подтверждении владельца, но нельзя утверждать, что измерение было именно под 25G241. C5 1,137,474,216 — историческая checkpoint; `I_C5_historical/R = 3,150.8036` user-CPU-s (примерно 52 min 31 s) — только проекция при этой rate, не wall time и не completed Linux boot. Stage 4 позднее достиг C7, а диагностический S4-38L7w (не засчитывается) достиг retired 1,185,015,387 и остановился на `IN AL, DX` ([план этапа 4](plans/STAGE_4.md), [evidence](reports/data/stage4/s4-38l7w-diagnostic-run.json)). Отдельный release sample сообщает ≈2.8M instr/s на 1.1B run; это иной прогон и профиль, не смешиваемый с S4-13 rate. `S4-39` остаётся открытым, поэтому фактические `I_boot`/`W_boot` до гостевого success marker отсутствуют. Сценарии C/D/G ещё не имеют измеренного `I_cycle`/`W_cycle`, и в документах стенда не задан целевой `B_soak`. Без этих входов формулы `N = floor(B_series/W_boot)` и `D_soak_estimate = ceil(B_soak/W_cycle)*W_cycle` не дают защищаемых значений; произвольные числа нарушили бы S5-01 и не доказывали бы S5-15/S5-16.
- **Условия и границы:** reduced checkpoint 200,000,000 instructions соответствует 554 s user CPU на измеренном S4-13 profile; он может проверить bounded diagnostic evidence path, но не завершает Linux boot, не удовлетворяет repeatability и не является soak. Основной Apple M3 Max/macOS и физический Intel Mac mini/Ubuntu — разные machines, их времена не смешиваются. Estimate `D_soak_estimate` не является фактическим elapsed: time gate фиксирует фактические завершённые cycles и elapsed после достижения `elapsed >= B_soak`.
- **Следующий шаг:** Method gate принят как subgate и разрешает только подготовку/функциональные проверки, если их независимые prerequisites пройдены; S5-08 остаётся blocked до S4-39/S4-31, S5-09 — до S7-16/PCI/IRQ, C/D/G — до своих S5-09/S5-21 gates. После принятия S5-08 измерить `I_boot`/`W_boot` на отдельном calibration run; после принятия повторяемого C+D+G cycle в S5-10/11/14 измерить `I_cycle`/`W_cycle`; заранее обосновать `B_series` и `B_soak`. Numeric gate вычисляет `N` и `D_soak_estimate`, и только его принятие завершает S5-01. Затем можно запускать S5-15/S5-16; их результаты не используются для определения собственного заранее требуемого `N`/budget. При принятом S5-16 time gate останавливается после текущего цикла при `elapsed >= B_soak`, с записью фактических cycles и elapsed. Не переносить сюда release/D3 budgets другого профиля и не называть `user CPU` wall time.

<a id="issue-075-s3-56-historical-corpus-comparability"></a>

### ISSUE-075 — S3-56: исторические cold и warm измерения используют разные корпуса

- **Дата обнаружения:** 2026-11-11, при выполнении S3-56 в worktree `codex`.
- **Компонент:** `tools/bench` first-run benchmarks и [отчёт S3 Core](reports/STAGE_3_CORE_REPORT.md).
- **Владелец:** компонент `tools/bench`, задача S3-56.
- **Статус:** открыта; native-only измерение на пяти S3-14 fixtures выполнено, полное сопоставление с S3-35a на его входе не подтверждено.
- **Основание и последствия:** S3-35a cold использовал замороженные 1 010 S3-17 cases и 1 322 visits; S3-36b warm — пять S3-14 cases и пять visits. План S3-56 одновременно требует один корпус и прямое сравнение с обоими историческими диапазонами. S3-17 `work/tmp/stage3/s3-17/cases.txt` и `dispatch-trace.json` отсутствуют в `codex` и основном checkout; их ожидаемые SHA-256 задокументированы в [S3-35a](reports/STAGE_3_S3-35a_2026-10-30.md). Числа, полученные на пяти случаях, нельзя использовать как прямое сравнение с cold 1 010-case sweep.
- **Поиск восстановления (2026-11-11):** при исходной проверке каталог `work/tmp/stage3` отсутствовал во всех 52 зарегистрированных worktree; stash был пуст. При повторной проверке ревьюера worktree было 53, каталог отсутствовал во всех. `git rev-list --all --objects` не обнаружил исходных путей `cases.txt` и `dispatch-trace.json`. Проверены SHA-256 1 561 недостижимого Git blob размером до 4 MiB и все 13 blob размером около 574 KiB во всём локальном Git object store: совпадений с ожидаемыми SHA-256 list и trace нет. Это ограниченный локальный поиск, а не доказательство отсутствия других копий.
- **Возможность восстановления:** исторический commit `b94171bf` доступен и содержит 1 895 файлов `tests/fixtures/x86`, но сохранённые отчёты дают агрегаты и хэши, а не перечень 1 010 выбранных путей и последовательность 1 322 visits. Повторный [`record`](../tools/bench/src/record.rs) записывает время исполнения и capture metadata; он не гарантирует побайтовое совпадение и исходный замороженный SHA-256 trace. Новый воспроизводимый corpus с новыми cold/warm baseline возможен, но не восстанавливает исторический вход и не делает измерения напрямую сопоставимыми с S3-35a.
- **Следующий шаг:** искать исходные S3-17 list и trace в резервной копии, архиве или на другой машине, проверить их по сохранённым SHA-256, выполнить generic cold на этом же corpus и сохранить raw результаты; для требования одного общего corpus отдельно уточнить план либо получить cold/warm baseline на идентичном наборе. Не подменять исторический вход новым корпусом и не приписывать измерению native-only выполнение mixed fallback S3-55.

<a id="issue-074-s3-55-mixed-fallback-witness"></a>

### ISSUE-074 — S3-55: строгий witness не представляет смешанный native/fallback поток

- **Дата обнаружения:** 2026-11-11, архитектурный анализ S3-55.
- **Компонент:** `od-jit` register admission и interpreter fallback ([`block.rs`](../crates/od-jit/src/block.rs), [`dispatch.rs`](../crates/od-jit/src/dispatch.rs)).
- **Статус:** исправлена для ограниченного mixed witness: точные bare CLD/STD и NOP; строгий witness не изменён. Общий Stage 2 fallback остаётся вне этого исправления.
- **Суть и последствия:** `NativeRegisterV1Admission::from_code` сканирует весь `CodeView` и возвращает `UnsupportedInstruction` на первой форме, которой отказал register classifier (`block.rs:399–455, 1087–1094`). Поэтому для смешанного потока native → bare CLD/STD → native строгий witness не создаётся и текущий generic dispatcher не получает вход для требуемого fallback. При этом `step_admitted_fallback` допускает для `NativeRegisterV1` только уже допущенный NOP (`dispatch.rs:4145–4185`); снятие этого guard само по себе не создаёт смешанный witness.
- **Основание и условия:** анализ действующих исходников в `codex` на `6c438a8`, подробно в [отчёте S3 Core](reports/STAGE_3_CORE_REPORT.md#8-s3-55--interpreter-fallback). `CodeMemory::one_instruction` ограничивает fetch одним байтом и запрещает data-memory access (`block.rs:8033–8076`); существующий admitted fallback для выбранных известных форм использует `bounded_range` с известной длиной и `od_isa::execute::step` (`dispatch.rs:4194–4215`). Это описание границ кода, а не результат выполнения mixed-корпуса. Формы, требующие отдельного execution profile или иного входа, в таком fallback остаются неподдержанными.
- **Результат и следующий шаг:** реализован opt-in witness и повторная проверка context/source/dependencies, mixed-корпус совпадает с интерпретатором на ARM64; команды и ограничения приведены в [отчёте S3 Core](reports/STAGE_3_CORE_REPORT.md#8-s3-55--interpreter-fallback). Независимое Sol-ревью кода, тестов и документации пройдено.

<a id="issue-055-s9-host-durability-guarantee-unconfirmed"></a>

### ISSUE-055 — Host durability guarantee for macOS/APFS is unconfirmed

- **Дата обнаружения:** 2026-11-05.
- **Компонент:** host storage backend `od-host` ([`storage_io.rs`](../crates/od-host/src/storage_io.rs)) / `od-storage` ([`io.rs`](../crates/od-storage/src/io.rs)) (S9-03, S9-06), контракт [STORAGE_FORMAT.md §7](specs/STORAGE_FORMAT.md#7-host-durability-api--статус-частично-реализовано-не-подтверждено-испытаниями).
- **Статус:** открыта. Это неподтверждённая платформенная гарантия, а не обнаруженный runtime-дефект. Обновлено после коммитов `a59338a`…`69e0b02`: flush данных файла на macOS реализован через `fcntl(F_FULLFSYNC)` (`69e0b02`), но не проверен испытаниями; открыто для namespace durability, power-loss и non-APFS/network томов.
- **Суть и последствия:** для выбранной кандидатной комбинации (macOS Tahoe 26.7.1, APFS, внутренний SSD) источник семантики сброса содержимого файла есть (локальная man `fcntl`, `F_FULLFSYNC`), но нет проверяемого источника и реализации durability namespace (create, rename, directory entry), а исполнение на конкретном накопителе не проверено испытаниями. Без этого нельзя доказывать порядок data → metadata → publication → ack и заявлять power-loss свойства. Реализованный file-data flush не доказывает namespace durability и power-loss; S9-06 не принята, claims durability за пределами file-data не допускаются.
- **Основание:** локальная man-страница `fcntl(2)`, macOS 26.7.1 (проверено 2026-11-05) — источник семантики `F_FULLFSYNC`: `fsync` плюс запрос накопителю сбросить буферы, barrier для ранее `fsync`-нутых данных, реализован на APFS; некоторые FireWire-накопители игнорируют запрос. Подтверждённым остаётся только это; сопоставление ENOTSUP/EINVAL man-страница не описывает (защитное предположение). Context7 Rust std (`/websites/doc_rust-lang_stable_std`): `File::sync_all` — «attempts to sync … to disk», `sync_data` может не синхронизировать metadata; namespace durability не описана. Источников по `F_BARRIERFSYNC` и namespace durability APFS нет. Правило плана [этапа 9 §3, п. 3](plans/STAGE_9.md#3-правила-исполнителя-и-размещение) запрещает угадывать гарантию.
- **Реализовано (факт кода):** `F_FULLFSYNC` на заблокированном writable fd только на macOS; ENOTSUP -> `UnsupportedDurability`, прочие сбои (включая EINVAL) -> sticky `reconcile_required`. Сопоставление ENOTSUP/EINVAL — защитное предположение, не подтверждённое man `fcntl`.
- **Следующий шаг:** получить проверяемый источник (официальная документация Apple/man pages конкретной версии), реализовать и проверить namespace durability, затем испытания уровней 1–3 по [STORAGE_RECOVERY.md](testing/STORAGE_RECOVERY.md). До этого durable операции для namespace отвечают `unsupported_durability`.

<a id="issue-056-s9-cpu-state-not-single-owner"></a>

### ISSUE-056 — Architectural CPU state has no single versioned owner

- **Дата обнаружения:** 2026-11-05.
- **Компонент:** `od-isa::LongModeState`, `od-isa::MsrProfile`, `od-machine::Long64ProbeConfig`; будущий S9-24.
- **Статус:** открыта; архитектурная граница для checkpoint, не runtime-дефект текущих bounded срезов.
- **Суть и последствия:** [`LongModeState`](../crates/od-isa/src/state.rs) несёт многие поля как `Option`, заполняемые только отдельными профилями, и не содержит CR2, CR3, CR8, EFER, DR*, LDTR, TR, XSAVE/AVX, general MSR и pending event state. CR3/EFER передаются отдельно через [`Long64ProbeConfig`](../crates/od-machine/src/page_walk.rs) и синтетический `MsrProfile::EferReadOnlyV1` ([`msr.rs`](../crates/od-isa/src/msr.rs)). Versioned codec отсутствует (только `Clone`/`PartialEq`). Полный CPU checkpoint по плану этапа 9 требует одного owner всего объявленного архитектурного состояния в `od-isa`; сейчас его нельзя собрать из одного объекта без потери полей или подстановки defaults.
- **Основание:** инвентаризация S9-01 ([VM_STATE.md](specs/VM_STATE.md#22-cpu-ram-устройства-время)), `rg` по `crates/od-isa/src/state.rs` 2026-11-05. Предположение, требующее проверки при S4-12/S4-18…21: возможно, часть состояния намеренно остаётся probe-конфигурацией до полного lifecycle.
- **Следующий шаг:** при закрытии этапа 4 задать единый versioned CPU state профиля в `od-isa` и его validation (S9-24); до этого full-VM checkpoint заблокирован.

<a id="issue-057-s9-cow-zeroed-gen2-slot-indistinguishable"></a>

### ISSUE-057 (закрыто) — S9-10 CoW: обнулённый слот поколения 2 вместе с повреждённой копией корня неотличим от обрыва до барьера 1 первого commit

- **Дата обнаружения:** 2026-11-05.
- **Компонент:** `od-storage`, прототип CoW ([`cow_proto.rs`](../crates/od-storage/src/cow_proto.rs), коммит `61a99e0`; тесты [`cow_open_tests.rs`](../crates/od-storage/src/cow_open_tests.rs)).
- **Статус:** закрыто (2026-11-05), коммит `01ef35d`. Формат CoW при этом по-прежнему не принят (S9-40).
- **Суть и последствия:** если слот подтверждённого поколения 2 обнулён, а копия корня поколения 2 в конце файла тоже повреждена, байты файла совпадают с состоянием после обрыва до первого barrier первого commit (поколение 1 есть, хвост без валидного корня). Writable open трактует это как `UnfinishedCandidate` на поколении 1, и следующий commit может затереть подтверждённое поколение 2: потеря acked данных при двойном повреждении. Salvage сообщает случай как незавершённый кандидат и ничего не пишет.
- **Основание:** тест в `cow_open_tests.rs` и комментарий к нему (строки около «zeroed slot of generation 2 … indistinguishable»); описание в сообщении коммита `61a99e0`.
- **Решение и результат:** `create` пишет два пустых корня (поколение 1 в slot 1 и поколение 2 в slot 0) под одним барьером; первый пользовательский commit — поколение 3. После подтверждённого `create` пустой слот — повреждение, а не норма. Спекулятивная ветка «первый commit» удалена. Один валидный пустой корень в файле размером с заголовок — незавершённый create: `CowError::IncompleteCreate` для writable open (read-only — `NoValidRoot`). Writable open с повреждённым новейшим слотом и повреждённой копией корня отклоняется (`DamagedOtherSlot`); salvage показывает предыдущее поколение с `possible_lost_commit`. Проверено sweeps по каждому cut create и commit против независимого oracle: `create_makes_two_empty_roots_under_one_barrier`, `power_cut_at_every_create_step`, `every_sector_state_of_an_unfinished_create`, `damage_to_one_or_two_root_sectors_never_yields_an_older_writable_state`, `zeroed_slot_of_the_first_commit_is_a_loss_or_a_roll_forward` ([`cow_open_tests.rs`](../crates/od-storage/src/cow_open_tests.rs)); `cargo test -p od-storage` — 114 passed. Ограничение: двойное повреждение вне модели отказов по-прежнему обнаруживается и отклоняется, но не восстанавливается.

<a id="issue-073-s2-46-fs-gs-base-on-null-load"></a>

### ISSUE-073 — S2-46: база FS/GS при null-загрузке не сверена с Intel

- **Дата обнаружения:** 2026-11-10, при ревью S2-46–S2-48.
- **Компонент:** `od-isa`, набор CPL0 (`MOV FS/GS, AX = 0`), `crates/od-isa/src/execute.rs`; контракт [CPU_CONTRACT.md](specs/CPU_CONTRACT.md) («Нулевая загрузка сегментов на CPL0 (S2-46)»).
- **Статус:** открыта; владелец — этап 2 (CPU-профиль Linux) / S4-39.
- **Суть и последствия:** база FS/GS при null-загрузке: набор (S2-46) сохраняет базу (поведение AMD без `NullSelectorClearsBase`); Intel обнуляет скрытую базу (по памяти, не сверено); CPUID набора — вендор OpenDesktop, Linux не выставит `X86_BUG_NULL_SEG`, и `load_seg_legacy` в `__switch_to` полагается на обнуление базы.
- **Основание:** ревью S2-46–S2-48; AMD документирует поведение CPUID-битом Fn8000_0021_EAX[6] `NullSelectorClearsBase`, набор этот бит не объявляет.
- **Следующий шаг:** решить до userspace (S4-39): выбрать поведение Intel или завести явный контракт.
- **Обновление (S4-38L6b):** в наборе появилась `WRMSR`/`RDMSR` `IA32_FS_BASE`/`IA32_GS_BASE` (минимальная модель: значение MSR в `LongModeState`); вопрос о null-загрузке селектора это не решает, статус не меняется.

<a id="issue-072-s4-29f-pump-trace-limit-periodic-pit"></a>

### ISSUE-072 — S4-29f: предел трассы насоса ограничивает прогон с периодическим источником

- **Дата обнаружения:** 2026-11-09, при реализации и ревью S4-29f.
- **Компонент:** `crates/od-machine/src/interrupt_pump.rs` (`INTERRUPT_PUMP_TRACE_LIMIT = 512`), мост `PitIrq0Owner` (`stage4_pit_irq0_probe.rs`).
- **Статус:** открыта; ограничение, не дефект среза.
- **Суть и последствия:** каждый период PIT добавляет в трассу несколько событий (idle_advance, фронт OUT0, planning, вход/EOI), поэтому бесконечная цепочка заканчивается `Rejected(TraceLimit)` примерно через 170 периодов (инъекции MaskedEntry2/NoEoi — при бюджете выше примерно 208 и 219 шагов). Для загрузки Linux это остановит run_boot. Срок хоста тоже ждётся при векторе, заблокированном в ISR (аналог ISSUE-070 для хоста); на железе время идёт, меняется только классификация исхода.
- **Основание:** контракт S4-29f («Модельные отказы и ограничения»), отчёт S4-29f.
- **Следующий шаг:** в S4-37a сделать трассу кольцевой или отключаемой для run_boot; решить классификацию простоя при заблокированном векторе хоста.
- **Обновление (S4-37a5):** закрыто кодом: кольцевая трасса `Ring(n)` с `trace_dropped` и сквозным `seq`; периодический PIT идёт дальше 1000 периодов в Ring без `TraceLimit`; `Bounded` без изменений. Открыто: `run_boot` пока не включает Ring сам (это S4-38, через `set_trace_mode(Ring(n))`); классификация простоя при заблокированном в ISR векторе хоста; выбор `n` для S4-38.

<a id="issue-071-s4-34a-kvm-method-single-run"></a>

### ISSUE-071 — S4-34a: KVM-метод подтверждён одним прогоном одного образа

- **Дата обнаружения:** 2026-11-09, при реализации S4-34a.
- **Компонент:** `tools/kvm-runner/kvm_runner.c`, `tools/build/build_stage4_kvm_method.py`, `docs/reports/data/stage4/s4-34a-kvm-method.json`.
- **Статус:** закрыта 2026-11-14 для исходного риска единичного прогона: S4-34b подтвердил три повтора каждого method case, а S4-34cde — три повтора всех 14 архитектурных cases, с независимым ревью и стабильной нормализованной CPUID identity. Повторяемость после host reboot не проверялась и остаётся ограничением; parent S4-34 остаётся открытым из-за prerequisites S4-00/S4-01.
- **Суть и последствия:** исходное S4-34a фиксировало по одному запуску на случай, без повторяемости между перезагрузками хоста; irqchip/APIC/таймер отсутствуют; исполнение — VT-x под KVM, не bare-metal; identity и вывод получены по ssh без подписи. S4-34b сохраняет сырые CPUID entries, передаваемые в `KVM_SET_CPUID2` из `KVM_GET_SUPPORTED_CPUID`, и host microcode; требует корректную структуру CPUID и обязательные host identity fields; повторяет каждый case; проверяет injection-specific exact I/O/HLT traces against UART text/hex, runner source and compiled-runner stability, plus image hashes around every run; rejects unstable guest outcomes/identity. Source hash сравнивался с локальным файлом до compilation; hash скомпилированного runner проверялся только на неизменность до/после каждой попытки, без сравнения с локальным бинарником. Сырые CPUID значения каждого прогона сохраняются. Стабильное сравнение нормализует только CPUID.1:EBX[31:24] и CPUID.0B:EDX, которые содержат logical-processor APIC IDs. Первый hardware run показал изменение этих полей между повторами при неизменных остальных CPUID entries. `method_status: accepted` нельзя трактовать как приёмку S4-34.
- **Основание:** отчёт [S4-34a](reports/STAGE_4_S4-34A_2026-11-09.md), раздел «Что не проверено и граница».
- **Основание обновления (2026-11-14):** [S4-34b report](reports/STAGE_4_S4-34B_2026-11-14.md) и [evidence](reports/data/stage4/s4-34b-kvm-repeatability.json) фиксируют две серии по три повтора каждого case. Первая показала различия только в per-CPU APIC-ID полях; во второй все девять запусков имели одинаковую нормализованную CPUID identity, стабильные исходы/выводы и совпавшие hashes образов до/после; source hash совпал с локальным файлом до compilation, runner binary hash оставался неизменным до/после каждой попытки. `main` завершился HLT и `OK\n`, `no_hlt` — wall deadline и `OK\n`, `wrong_str` — HLT и `NO\n`; два injection cases были корректно помечены как ожидаемо отличающиеся от baseline, с точным соответствием I/O trace и UART. Offline suite — 41 tests passed, включая CPUID normalization, malformed/mismatched injection traces, image/source/binary hash mismatches and post-run mutation.
- **Основание обновления (2026-11-14):** [S4-34b report](reports/STAGE_4_S4-34B_2026-11-14.md) и [evidence](reports/data/stage4/s4-34b-kvm-repeatability.json) фиксируют три повтора каждого method case. [S4-34cde report](reports/STAGE_4_S4-34CDE_2026-11-14.md) и [evidence](reports/data/stage4/s4-34cde-kvm-system.json) фиксируют 42/42 принятых архитектурных KVM invocation для 14 cases; builder сохраняет raw CPUID/CPU state, result blocks и per-attempt hashes, проверяет CPUID identity, result semantics и repeat stability. Unit suite S4-34cde — 9 tests passed.
- **Следующий шаг:** для ISSUE-071 не требуется. Хост reboot repeats не выполнялись. Parent S4-34 можно пересмотреть после закрытия S4-00 и S4-01.

<a id="issue-070-s4-27i2-idle-advance-burns-budget-when-timer-vector-blocked"></a>

### ISSUE-070 — S4-27i2: idle advance сжигает бюджет, когда вектор таймера заблокирован

- **Дата обнаружения:** 2026-11-09, при реализации и ревью S4-27i2.
- **Компонент:** `crates/od-machine/src/interrupt_pump.rs` (`InterruptPump::idle_advance`, `FlatBackend::step_waiting`, `idle_flow`).
- **Статус:** открыта; не покрыта тестом.
- **Суть и последствия:** если CPU в HLT при IF=1, а вектор периодического таймера не может стать eligible (он в ISR без EOI, например handler выполнил STI и HLT до EOI, либо его блокирует TPR), pump на каждом шаге продвигает часы на период, а повторный expiry схлопывается в уже взведённый IRR. Выход — `Budget`, а не `IdleHalt`; виртуальное время уходит на budget·P. Состояние гостя корректно (на железе время тоже идёт), неверна только классификация простоя.
- **Основание:** отчёт [S4-27i2](reports/STAGE_4_S4-27I2_2026-11-09.md), разделы «Отклонения» и «Ограничения».
- **Следующий шаг:** в `idle_advance` возвращать `IdleHalt`, если вектор таймера уже в IRR и не eligible; добавить тест. Владелец — S4-27i3 (вместе с арбитражем очереди).

<a id="issue-068-s4-27i1-partial-rep-interrupt-latency"></a>

### ISSUE-068 — S4-27i1: прерывание не принимается внутри частичного REP

- **Дата обнаружения:** 2026-11-09, при реализации S4-27i1.
- **Компонент:** `crates/od-machine/src/interrupt_pump.rs` (`FlatBackend::step_running`, `mid_rep` теперь в бэкенде), `crates/od-machine/src/diagnostic_vcpu.rs` (`completed_boundary`, `run_apic_quantum`).
- **Статус:** открыта; отклонение среза от плана, не дефект логики.
- **Суть и последствия:** после `WorkLimit(RepIterationBudget)` (1024 итерации) граница не завершена, принятые примитивы доставки её отвергают, и pump довершает REP через `run(1)` без проверки APIC. Прерывание принимается только после REP (RIP кадра — следующая инструкция), тогда как железо может принять его между итерациями. Состояние гостя корректно, растёт только задержка, пропорциональная длине REP.
- **Основание:** тест `partial_rep_delays_the_interrupt_until_the_instruction_completes` (`crates/od-machine/tests/interrupt_pump_flat.rs`), отчёт [S4-27i1](reports/STAGE_4_S4-27I1_2026-11-09.md).
- **Следующий шаг:** определить владельца приёма IRQ между итерациями REP (кандидаты — S4-27i3 или S4-38). До этого не заявлять приём «на каждой границе» для REP.

<a id="issue-069-s4-27i1-pump-inherits-narrow-sti-language"></a>

### ISSUE-069 — S4-27i1: pump наследует узкий язык STI из S4-27c

- **Дата обнаружения:** 2026-11-09, при ревью S4-27i1.
- **Компонент:** `crates/od-isa/src/execute.rs` (`StartIfSet`, около 2272–2281), `crates/od-machine/src/interrupt_pump.rs`.
- **Статус:** открыта; владельца в плане нет.
- **Суть и последствия:** принятый путь S4-27c допускает после `sti` только NOP, STI, CLI и HLT. `cli` при IF=1 вне STI-shadow и любая другая инструкция в shadow дают terminal `Unsupported`, и pump на таком образе завершается. Linux `local_irq_enable/disable` на это наткнётся. Подсрезы S4-27i1–i5 этот профиль не расширяют.
- **Следующий шаг:** назначить владельца (вопрос к консультации Opus) и решить, расширять ли профиль STI до S4-37a или оставить узким до Linux-профиля.

<a id="issue-067-s7-14a-pm-timer-frozen-inside-a-quantum"></a>

### ISSUE-067 — S7-14a: PM-таймер стоит внутри кванта

- **Дата обнаружения:** 2026-11-08, при ревью S7-14a.
- **Компонент:** `crates/od-machine/src/acpi_pm.rs` (`PM_TMR`), `firmware_board.rs` (`LoadedFirmwareBoard::run_boot`, `advance_time_ns`).
- **Статус:** открыта; ограничение среза, не дефект логики PM-блока.
- **Суть и последствия:** `PM_TMR` читает снимок virtual clock платы, а часы двигаются только между квантами (`advance_time_ns(report.instructions_executed)`). Квант заканчивается на записи в COM1 THR, doorbell NVMe, запросе остановки PM, `HLT` или конце бюджета. Гость, который опрашивает `inl(0x608)` без этих событий, видит одно и то же значение до конца бюджета: ожидание по `PM_TMR` (UEFI `Stall`, калибровка TSC/таймеров в Linux) не завершится до конца бюджета.
- **Основание:** чтение `run_boot` (часы двигаются после `self.run`), тест `the_pm_timer_follows_the_board_clock` и тест `run_boot_advances_the_clock_by_retired_instructions_and_pm_tmr_sees_it`. Первый проверяет, что PM_TMR следует часам платы; второй — что `run_boot` продвигает часы на отработанные инструкции. Неподвижность таймера внутри кванта ни один тест не показывает, она следует из чтения `run_boot`.
- **Следующий шаг:** закрывать квант на чтении `PM_TMR` (или на опросе по счётчику повторных чтений) либо двигать часы внутри кванта; решение за S4-37/S4-38. Механизм квантов в S7-14a не менялся.
- **Обновление (S4-29d, 2026-11-09):** порт-тик Δ = 1 мкс на каждое чтение `0x61` сделан только в профиле `Stage4PitCalibrateProbeV1` (отчёт [S4-29d](reports/STAGE_4_S4-29D_2026-11-09.md)); в `LoadedFirmwareBoard::run_boot` и `PagedVcpuSession` очередь тика не применяется, ISSUE-067 не закрыта.
  - Открыто: тик на `0x42` и `PM_TMR`; применение очереди тика в `run_boot` и `PagedVcpuSession`; синхронизация часов по выполненным инструкциям (сейчас инструкция = 0 нс в профиле).
  - Следствие: Linux `quick_pit_calibrate` (`pit_verify_msb`, живые чтения `0x42` без `0x61`) на этой модели время не продвинет и не завершится.
  - Перенос закрытия: по рекомендации Opus (ревью S4-29d) закрытие ISSUE-067 переносится в **S4-37a** (PIT и PM-блок на общих часах и насосе событий); решение зафиксировано в плане (п. 5 «Решения по консультации Opus»).
- **Обновление (S4-37a2/a3, 2026-11-09):** закрыто: тик PM_TMR (на всех платах с `acpi_pm`), тики `0x61` и `0x42` (под `linux_profile`), применение очереди PIT и PM_TMR в `LoadedFirmwareBoard::run_boot`. Открыто: `PagedVcpuSession` и прохождение полной `quick_pit_calibrate` (требуют CMP/Jcc в paged-профиле); переносится в a6 и S4-38. Статус остаётся «открыта».

<a id="issue-066-s7-16-nvme-spec-values-not-verified"></a>

### ISSUE-066 — S7-16: значения NVMe в `od-machine` частично сверены (Linux `nvme.h`, не спецификация), остальное не сверено, I/O не реализован

- **Дата обнаружения:** 2026-11-07, при реализации S7-16 (срез A).
- **Компонент:** `crates/od-machine/src/nvme/`, [NVME_DEVICE.md](specs/NVME_DEVICE.md).
- **Статус:** открыта.
- **Суть и последствия:** текст NVMe Base 2.4, NVM Command Set 1.3 и PCIe Transport 1.4 получить не удалось (страница спецификаций не даёт прямых ссылок, PDF 2.4 не найден, PDF 2.3 не извлекается через `WebFetch` и индексатор). Значения, перечисленные в [NVME_DEVICE.md §1](specs/NVME_DEVICE.md), сверены с заголовком Linux `include/linux/nvme.h` (master, 2026-11-07), **не** с текстом NVMe Base 2.4. NOT VERIFIED остаются: `CAP.CQR` (бит 16), AER info 0/1, правила PRP, умолчания features, поведение при ошибках, обязательность команд, LID `0x00` и `0x05`, `VS` 2.0.0 против 2.4.0. Открытые вопросы среза A: (1) проверка `IOSQES`/`IOCQES` при `CC.EN = 1` (`CFS`) против проверки при Create I/O SQ/CQ — вероятное отклонение (`controller.rs`, `enable`); (2) Supported Log Pages (`0x00`) и Commands Supported and Effects (`0x05`) не реализованы, обязательность не сверена — первый кандидат в дефекты; (3) `VS = 2.0.0`; (4) запись doorbell гостем не покрыта тестом end-to-end (см. NVME_DEVICE.md §3). Кроме того, срез A не реализует обязательные I/O-команды `Read`/`Write`/`Flush` (срез B): контроллер не читает диск, прошивка (S7-17) им воспользоваться не может.
- **Основание:** [STAGE_7_REPORT.md](reports/STAGE_7_REPORT.md), раздел S7-16; NVME_DEVICE.md §1, §10, §11.
- **Следующий шаг:** до ревью среза A и до S7-17 получить текст спецификаций и сверить значения, помеченные NOT VERIFIED (NVME_DEVICE.md §11); решить вопрос `IOSQES`/`IOCQES`; реализовать срез B; после сверки снять пометки `NOT VERIFIED` в исходниках и документе.

<a id="issue-065-s7-dsdt-pci0-has-no-io-mmio-windows-or-interrupt-routes"></a>

### ISSUE-065 — S7-12: DSDT PCI0 has no I/O, MMIO windows or interrupt routes

- **Дата обнаружения:** 2026-11-07.
- **Компонент:** handoff block v1.0 ([FIRMWARE_BOOT.md §6.3](specs/FIRMWARE_BOOT.md)), DSDT из manifest ([`dsdt.rs`](../firmware/uefi/src/acpi/dsdt.rs), S7-12).
- **Статус:** открыта; ограничение контракта, не runtime-дефект (без флага платы `pci_ecam` код 7 остаётся `PRESENT = 0` и PCI0 не выводится; с флагом `pci_ecam` (S4-30c) код 7 — `PRESENT = 1`, DSDT выводит `PCI0` с одним `WordBusNumber` и пустым `_PRT`, и это ограничение становится достижимым; MCFG, `_CRS` с окнами, host bridge и функции — S4-31/S4-38/S7).
- **Суть и последствия:** запись PCIe ECAM (тип 7) несёт только базу и длину ECAM, сегмент и диапазон шин. Окон I/O и MMIO корневого моста и маршрутов прерываний PCI в handoff block нет, а DSDT строится только из него. `_CRS` `\_SB.PCI0` поэтому содержит один `WordBusNumber`, `_PRT` — пустой пакет. ОС не получит от ACPI окон для назначения BAR и маршрутов INTx устройств PCI (NVMe S7-16, xHCI, сеть).
- **Основание:** чтение FIRMWARE_BOOT.md §6.3 и `docs/specs/VIRTUAL_BOARD.md` (окна HPET и ACPI fixed hardware «не назначены, S7-14»); окно ECAM выбрано в S4-30a, под флагом `pci_ecam` код 7 `PRESENT = 1` (S4-30c, MACHINE_CONTRACT.md); тесты `dsdt_for_pci_root_and_com1_matches_the_independent_reference` и `ecam_entry_emitted_by_the_board_with_the_pci_flag_is_accepted`.
- **Следующий шаг:** при S4-30/S7-13 добавить окна и маршруты в handoff block (повышение `version_minor`, запись в FIRMWARE_BOOT.md) и вывести из них `DWordMemory`/`QWordMemory`/`IO` и `_PRT` в `dsdt.rs`; до этого не заявлять работу PCI-устройств в ОС по ACPI.

<a id="issue-064-s7-rsa-private-ops-not-constant-time"></a>

### ISSUE-064 — Selected `rsa` crate has no constant-time private operations (RUSTSEC-2023-0071)

- **Дата обнаружения:** 2026-11-07.
- **Компонент:** `crates/od-tpm/` (`src/crypto.rs`: `rsa_sign`, `rsa_generate`; приватные RSA-операции фазы B, S7-23); выбор зависимости — [ADR-0009](decisions/ADR-0009-vtpm-placement-and-crypto.md#условие-d9-о-constant-time-rsa).
- **Статус:** открыта, принятый риск по решению владельца D9 ([план этапа 7, §1.1](plans/STAGE_7.md#11-решения-владельца-проекта-2026-11-07)). **Код, использующий `rsa`, есть со срезом B S7-23 (2026-11-07, ожидает ревью):** `TPM2_Sign` с RSA-ключом и генерация RSA-ключей в `CreatePrimary`/`Create`; **со срезом C1 (2026-11-07, ожидает ревью) появилась `TPM2_RSA_Decrypt`** (RSAES, OAEP и «сырой» RSA; приватная операция `rsa` с blinding от DRBG — тот же модуль и те же риски); принятый риск действует. До замены на constant-time версию `rsa` операция `Sign` с RSA-ключом создаёт риск timing-атаки на host-процесс с vTPM (на прошивке это не стек, прошивка секретов не хранит); подпись вызывается с RNG для blinding (генератор TPM), constant-time этим не заявляется.
- **Суть и последствия:** D9 требует для приватных операций RSA на host версию `rsa` с constant-time арифметикой. На 2026-11-07 такой версии нет: последняя стабильная `0.9.10`, предрелиз `0.10.0-rc.22`; advisory RUSTSEC-2023-0071 (Marvin, утечка приватного ключа по времени) имеет `patched = []`. Выбран `=0.10.0-rc.22`. Marvin — атака именно на расшифровку RSAES PKCS#1 v1.5 по времени ответа, поэтому `RSA_Decrypt` (RSAES, а при формировании окна наблюдения — и OAEP/«сырой» RSA того же модуля) её прямая цель; принятый риск D9 действует. Затронуты `TPM2_Sign` с RSA-ключом (реализовано, срез B), `RSA_Decrypt` (реализовано, срез C1), а после среза C2 — salted `StartAuthSession`, `ActivateCredential`, `Import` с RSA-ключами; гостевой код может наблюдать время их исполнения. Проверка подписи в прошивке (операция с открытым ключом) не затронута.
- **Основание:** `rustsec/advisory-db`, `crates/rsa/RUSTSEC-2023-0071.md` (прочитан 2026-11-07): «Still affected as of 2026-09-12: rsa 0.9.10 (latest stable) and rsa 0.10.0-rc.18»; README `rsa 0.10.0-rc.22` ссылается на продолжающуюся работу по constant-time hardening; версии — crates.io API.
- **Следующий шаг:** `VTPM.md` («Фаза B, срез B» и «Фаза B, срез C1») записывает, что приватный RSA используют `Sign`, `RSA_Decrypt` и генерация ключей; стойкость к timing-атакам не заявляется (срез C2 добавит остальные команды в этот список). Закрыть, когда advisory получит `patched` или выйдет версия с подтверждённой constant-time реализацией: обновить версию в ADR-0009 и `Cargo.toml` с ревью.

<a id="issue-063-s4-secondary-page-fault-requires-double-fault-delivery"></a>

### ISSUE-063 — Secondary page fault during exception delivery is terminal

- **Дата обнаружения:** 2026-11-06.
- **Компонент:** `od-isa` page-fault delivery and `od-machine` paged exception adapter.
- **Статус:** решена в пределах S4-19c для фактического page fault во время bounded #PF/#DF delivery. Другие exception pairs и недоступность #DF handler из-за backend/profile failure остаются вне этого результата.
- **Суть и последствия:** ранее nested `#PF` оставлял только terminal diagnostic. Теперь page fault при доставке #PF вызывает ровно одну попытку #DF; page fault при доставке #DF приводит к `CpuActivity::Shutdown`. Частичные frame/page-walk эффекты остаются диагностическими и shutdown state не возобновляется.
- **Основание и проверка:** S4-19c проверяет read-only GDT с A=0 и успешный #DF handler, а также page fault в #DF stack delivery, latest CR2, retained writes и отсутствие дальнейшего fetch/effects. См. [S4-19c report](reports/STAGE_4_S4-19C_2026-11-06.md), [machine contract](specs/MACHINE_CONTRACT.md#secondary-pf-df-delivery-and-shutdown-s4-19c) и [verification record](testing/MACHINE.md#s4-19c-secondary-pf-double-fault-delivery-and-shutdown).
- **Остаток этапа:** полный S4-19 ещё требует guest PTE repair, соответствующей invalidation, IRETQ и точного retry. План: [S4-19](plans/STAGE_4.md#s4-19-доставка-исключений-и-восстановление-после-fault).

<a id="issue-054-s4-11-cross-page-cpu-integration"></a>

### ISSUE-054 — Cross-page, partial-execution and general rollback integration is not established

- **Дата обнаружения:** 2026-11-04.
- **Компонент:** `od-machine` tracked RAM access, `od-isa::GuestMemory` and other consumers that may rely on rollback.
- **Статус:** открыта; это не подтверждённый runtime-дефект, а незакрытая архитектурная граница. S4-06b и bounded S4-11b/c/d/e имеют независимые ревью; S4-11e принят после исправления двух P2 admission findings и одного P3 Graphify-evidence finding. Rollback других `GuestMemory` consumers не установлен.
- **Суть и последствия:** S4-06b фиксирует same-page guarantees для двух точных MOV, включая caller-buffer isolation, отсутствие частичного operand write и сохраняющиеся A/D effects при alias. S4-11b поддерживает только CPU-connected qword `MOV RAX,[RBX]`, а S4-11c — отдельный dword `MOV EAX,[RBX]`; каждый profile переводит обе части и проверяет разрешения/RAM до чтения operand, сохраняет A updates ранее завершённых walks и коммитит CPU destination только после полного read. S4-11d проверяет crossing fetch только для `8B 03` в существующем opt-in profile, а S4-11e добавляет отдельный exact word-read profile для `66 8B 03`; ни один срез не обобщает поддержку на другие формы. Общий `access_long64_4k_ram` сохраняет ранний cross-page refusal; расширения ограничены новыми CPU paths. S4-11f adds independently reviewed bounded word-MOV instruction fetch across code pages; its 33 integration, 82 machine unit and 3 focused ISA tests pass. This does not close the general contract. Полного RAM rollback и широкого cross-page/device/restart контракта нет.
- **Основание:** S4-11b прошёл 15 integration tests, 82 unit tests и независимое review. S4-11c проверяет same-page control, все три dword crossing offsets, second-page #PF, zero-extension, alias, no-D и отсутствие MMIO dispatch; `gpt-6.1-sol/high` review одобрило срез после исправления P3 evidence несоответствия. S4-11d проверяет fetch через две code pages, PFEC 0x10/0x11, точный A-update journal, отсутствие operand walk при fetch fault, hole/MMIO без device callback и decoder boundary control; independent review завершено без оставшихся P0—P3. S4-11e additionally checks same-page and crossing word reads, CPL0/CPL3 second-page faults, unchanged CPU/caller buffer, A-prefix persistence, no D, holes/MMIO and profile exclusions; it has 26 passing integration tests, 82 machine unit tests, a passing shared CPU-profile gate test and independent review with no remaining P0—P3 findings. См. [S4-11e](reports/STAGE_4_S4-11E_2026-11-05.md) and [manifest](reports/data/stage4/s4-11e-cross-page-word-read.json). Результаты: [S4-11b](reports/STAGE_4_S4-11B_2026-11-05.md), [S4-11c](reports/STAGE_4_S4-11C_2026-11-05.md), [S4-11d](reports/STAGE_4_S4-11D_2026-11-05.md); локальные contracts: [machine](specs/MACHINE_CONTRACT.md#bounded-cross-page-instruction-fetch-s4-11d), [CPU](specs/CPU_CONTRACT.md#cross-page-fetch-for-pagedmovcrosspagedwordreadlong64v1-s4-11d). Related CPU restart uncertainties remain in [ISSUE-016](#issue-016--не-установлен-restart-state-movs-после-destination-fault) and [ISSUE-014](#issue-014--не-установлено-состояние-rsp-после-fault-у-memory-destination-pop).
- **Следующий шаг:** отдельно задать/проверить fault/effect contracts для cross-page stores, других widths, stack/REP/RMW и device boundaries; проверить rollback assumptions у остальных consumers. Не переносить ограниченные S4-11b/c доказательства на другие формы.

<a id="issue-034-s2-42e-execute-only-code-not-admitted-by-native-v2"></a>

### ISSUE-034 — S2-42e execute-only code native acceptance

- **Дата обнаружения:** 2026-11-04; разрешено 2026-11-04.
- **Компонент:** Linux x86_64 native-v2 mapping and S2-42e/S2-42q MOV acceptance.
- **Статус:** разрешено для bounded S2-42e positive MOV profile; hardware execute-only enforcement remains unclaimed.
- **Суть:** Initial `MappingPlan` rejected all fixtures with immutable logical execute-only code before instruction execution. The runner now projects to host RX only for one exact `LongIntegerV1` register↔memory MOV in one executable region, with budget 1, exact decoded next RIP, `StopAddress`, and successful existing initial admission. Fixture logical permissions remain authoritative for initial and per-step access checks. This does not claim hardware execute-only enforcement or guest `#PF` delivery.
- **Проверка и evidence:** Linux x86_64 run at source fingerprint `ccfd49a28ea6ddf804f2124dab30b0e569b7e1666a8808e0296b12f642f96f49` passed all 1,080 selected cases twice on interpreter and native; saved-run verification passed for 2,160 results per backend. Summary: [S2-42e run](../work/stage2-42e-oracle/s2-42e-summary.json); full records: `work/stage2-42e-oracle/stage2-20261104-120215-67d38a75/`. The initial closed failure remains in `work/stage2-42e-oracle/stage2-20261104-102111-498cc626/`. The behavior and limits are described in [CPU_ORACLE](testing/CPU_ORACLE.md) and the [S2-42e protocol](testing/S2_42E_MOV_MEMORY_CORPUS.md).
- **Граница:** physical Mac acceptance and hardware execute-only fault delivery remain `not_run`; those are separate acceptance work.

<a id="issue-035-linux-native-v2-broad-test-failures"></a>

### ISSUE-035 — Linux native_v2 broad test group had baseline failures

- **Дата обнаружения:** 2026-11-04.
- **Компонент:** `tools/x86-runner`, Linux x86_64 `native_v2` unit-test group.
- **Статус:** решено 2026-11-04.
- **Суть и последствия:** The initial `cargo test --offline -p od-x86-runner native_v2::tests -- --nocapture` group run had 50 identical failures on parent `286b70c` and S2-42e `0a0640b`. Triage found stale fixture/profile setup and denial expectations, plus a confirmed MAXPS memory-profile routing gap. The x87 fixture helper now uses the selected profile state; profile budgets, fixture names, nested RET cleanup, strict schema rejections, and current denial text are checked at their intended boundaries. Separately, MAXPS memory profiles now fail closed before native trace. No native SSE capability was added.
- **Проверка и evidence:** On Linux x86_64 host `192.168.1.39` as `senya`, `cargo test --offline -p od-x86-runner native_v2::tests -- --nocapture` passed **217 tests, 0 failed** (19 filtered). The tested source files match main revision `64290d2b643ff69912a5c9b3969081e9dbbe8bca`; the Linux checkout started at `0a0640b` and received only code changes to `native_fp.rs` and `native_v2.rs`, preserved as [source patch](../work/stage2-42e-oracle/native-v2-issue035-source.patch). The focused x87 denial matrix passed independently. Full resolved transcript: `work/stage2-42e-oracle/native-v2-issue035-resolved.log`. Initial transcripts remain at [native-v2-baseline.log](../work/stage2-42e-oracle/native-v2-baseline.log) and [native-v2-final.log](../work/stage2-42e-oracle/native-v2-final.log). The five suite evidence remains separately recorded in [S2-42e evidence](../work/stage2-42e-oracle/stage2-20261104-120215-67d38a75/).
- **Граница:** this closes the broad Linux native_v2 unit-test baseline only. Physical Mac acceptance, S2-39k ptrace oracle, and parent S2-42 acceptance remain open.

<a id="issue-036--ubuntu-fpu-source-image-mapping-artifacts-are-unavailable"></a>

### ISSUE-036 — Ubuntu FPU source/signature mapping reproduction is incomplete

- **Дата обнаружения:** 2026-11-04.
- **Компонент:** S2-00/S2-40 static mapping of Ubuntu Generic amd64 7.0.0-38.38 FPU initialization and save/restore alternatives.
- **Статус:** открыто; evidence-reproduction gap, not a confirmed source or implementation defect.
- **Суть и последствия:** The isolated S2-40d checkout has no `work/vms/s2-00-ubuntu-26.04.1/` artifacts. S4-00a recovered the exact-version source `.deb`, its extracted source-tree archive, selected source files and config into ignored work artifacts. It revalidated the four binary artifact digests against the tracked report, found matching Build IDs for `vmlinux` and debug ELF, and confirmed equality of 38 common allocated file-backed sections. The signed repository/source chain is still absent, and no authenticated source/config-to-binary or DWARF mapping is established. Prior static findings remain unauthenticated by this source mapping; no runtime selection is claimed.
- **Основание и воспроизведение:** Binary digest evidence is recorded in [S2-40d](reports/S2_40D_ARTIFACT_DIGEST_RECHECK.md). Source/config hashes, artifact paths, commands, comparison scope and non-claims are recorded in the [S4-00a report](reports/STAGE_4_S4-00A_2026-11-05.md) and [manifest](reports/data/stage4/s4-00a-source-audit-2026-11-05.json). The source `.deb` came from the recorded Huawei mirror URL. The signed `linux_7.0.0-38.38.dsc`, `linux_7.0.0.orig.tar.gz`, `linux_7.0.0-38.38.diff.gz`, `Sources.xz`, `InRelease` and `apt-provenance.json` were not recovered. `shasum -a 256`, `file`, a read-only Python 3 ELF64 section parser, `objdump -h`, `ar`, `zstd` and `tar` were used; the environment lacks `readelf`.
- **Следующий шаг:** recover the exact signed indexes, source package files and provenance record from an authenticated snapshot; verify signatures/hashes, then reproduce the full section/build-ID comparison and source/DWARF mapping. Keep S4-00/S4-01, runtime selection and boot claims open.

<a id="issue-033-s2-37cq-comiss-mixed-event-handling-audit"></a>

### ISSUE-033 — S2-37cq COMISS mixed-event handling needs audit

- **Дата обнаружения:** 2026-11-02.
- **Компонент:** `od-isa` COMISS comparison and the bounded S2-37cq/S2-37v profiles.
- **Статус:** открыто; расхождение с SDM или физическим x86 не установлено.
- **Суть и последствия:** S2-37cq и S2-37v задают локальную обработку mixed NaN/denormal operands. Нужен отдельный аудит raw classification, mask-dependent control flow и независимых oracle, чтобы подтвердить согласованность локальных профилей с применимыми правилами Intel SDM. S2-38u и S2-38v намеренно остаются invalid-only и отказывают после чтения при любом subnormal; этот узкий результат не разрешает аудит прежних профилей.
- **Основание:** Intel SDM Rev. 093 Vol. 2A перечисляет для COMISS invalid и denormal conditions; Vol. 1 §§4.9.2, 11.5.2.1–2 описывают precedence и SIMD exception behavior ([official PDF, Vol. 1](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=112), [Vol. 2A](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=319)). Это основание для аудита, а не подтверждение дефекта. Физические проверки не выполнялись. См. [S2-37cq](specs/CPU_CONTRACT.md#s2-37cq--bounded-long64-comiss-xmm0dword-ptr-rax-m32-memory-profile), [S2-37v](specs/CPU_CONTRACT.md#s2-37v--canonical-long64-comiss-and-ucomiss-xmm-register-pairs), [S2-38u](specs/CPU_CONTRACT.md#s2-38u--exact-comiss-memory-invalid-only-profile) и [S2-38v](specs/CPU_CONTRACT.md#s2-38v--exact-ucomiss-memory-invalid-only-profile).
- **Следующий шаг:** выполнить профильный аудит production paths и независимых schema/testkit oracles S2-37cq/S2-37v на NaN, subnormal и mixed operands; при доступности физического x86 добавить контрольные MXCSR cases. Регистрировать mismatch только если его подтвердят код, нормативный анализ или измерение.

### ISSUE-032 — S2-37dj lacks physical x86 acceptance evidence

- **Дата обнаружения:** 2026-11-02.
- **Компонент:** локальный `Long64` interpreter profile `s2-long-addsd-memory-rax-v1` в od-isa.
- **Статус:** открытое ограничение приёмочных свидетельств; подтверждённый дефект не установлен.
- **Суть и последствия:** masked-only `ADDSD` m64 [RAX] проходит raw-bit ISA и независимый fixture oracle, но физическое исполнение на x86 не сравнивалось. Локальные проверки не подтверждают аппаратную fault ordering, unaligned memory behavior или `MXCSR` status.
- **Основание:** реализация и границы описаны в [standalone report](reports/S2_37DJ_ADDSD_MEMORY.md) и [shared report](reports/STAGE_2_REPORT.md#s2-37dj--addsd-xmm0m64-rax); все доступные проверки остаются локальными.
- **Следующий шаг:** включить точные encoding/profile, AC, span faults и `MXCSR` scenarios в общий физический x86 acceptance substage; зафиксировать CPU/microcode, режим и результаты. До этого не заявлять physical acceptance.

### ISSUE-031 — S2-37di lacks physical x86 acceptance evidence

- **Дата обнаружения:** 2026-11-01.
- **Компонент:** локальный Long64 interpreter profile `s2-long-addss-memory-rax-v1` в `od-isa`.
- **Статус:** открытое ограничение приёмочных свидетельств; подтверждённый дефект не установлен.
- **Суть и последствия:** exact `ADDSS XMM0,m32 [RAX]` согласуется с существующим ADDSS raw helper и независимым integer fixture oracle, но физическое исполнение на x86 не сравнивалось. Локальные тесты поэтому не подтверждают аппаратный fault ordering, unaligned memory behavior или MXCSR status на целевом процессоре.
- **Основание:** S2-37di добавляет interpreter-only профиль и фиксирует отсутствие физической проверки в [standalone report](reports/S2_37DI_ADDSS_MEMORY.md) и [shared report](reports/STAGE_2_REPORT.md#s2-37di--addss-xmm0m32-rax); локальные suites подтверждают только выбранную семантику и oracle.
- **Следующий шаг:** включить точные encoding/profile, AC, boundary faults и MXCSR scenarios в общий физический x86 acceptance substage; сохранить CPU/microcode, режим и результаты. До этого не заявлять physical acceptance.

### ISSUE-030 — S2-37dh lacks physical x86 acceptance evidence

- **Дата обнаружения:** 2026-11-01.
- **Компонент:** локальный Long64 interpreter profile `s2-long-minss-memory-rax-v1` в `od-isa`.
- **Статус:** открытое ограничение приёмочных свидетельств; подтверждённый дефект не установлен.
- **Суть и последствия:** точное исполнение `MINSS XMM0,m32 [RAX]` проверяется локальными raw-bit ISA и fixture oracle, но на физическом x86 процессоре не сравнивалось. Поэтому локальные проверки подтверждают согласованность реализации с выбранным контрактом и oracle, но не аппаратное поведение по AC, memory faults и MXCSR.
- **Основание:** S2-37dh добавляет только interpreter-only профиль и фиксирует отсутствие physical x86 acceptance в [standalone report](reports/S2_37DH_MINSS_MEMORY.md) и [shared report](reports/STAGE_2_REPORT.md#s2-37dh--minss-xmm0m32-rax). Локальные focused/full suites подтверждают fixture и semantic matrix.
- **Следующий шаг:** включить этот точный encoding/profile в общий физический x86 acceptance substage; сравнить результат и fault ordering на целевой аппаратуре и записать конфигурацию и наблюдения. До этого не заявлять physical acceptance.

### ISSUE-029 — Linux x86 runner target did not compile all tests

- **Дата обнаружения:** 2026-11-01; исправлено 2026-11-03.
- **Компонент:** `tools/x86-runner`, Linux x86_64-only `native_v2` backend.
- **Статус:** Linux compile blocker решён 2026-11-03. Пять согласованных S2-42 suites и полный Linux `native_v2::tests` group прошли; общий group дал 217 passed / 0 failed после закрытия ISSUE-035. Отдельный S2-39k ptrace oracle и physical hardware остаются `not_run`.
- **Основание и последствия:** ранее `cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu --offline` обнаруживал два неполных Linux-only initializer-а в `native_v2.rs`: `Snapshot` и тестовый `Fixture`. Каждый не задавал 142 текущих optional control fields schema-v2. Все поля теперь явно заданы как `None`; наблюдаемое состояние, native admission policy и семантика не менялись. `serde_json` 1.0.151 уже присутствует в зависимостях runner и успешно компилируется для Linux target; отдельная dependency правка не потребовалась.
- **Воспроизведение и результат:** указанная cross-target команда после исправления завершается успешно для `--all-targets --offline`; сама по себе она подтверждает только type-check. Отдельные пять согласованных S2-42 native suites запущены на Linux x86_64. Broad `native_v2::tests` group прошёл: 217 passed / 0 failed; полные данные и exact source patch — в [ISSUE-035](#issue-035-linux-native-v2-broad-test-failures). S2-39k ptrace oracle не выполнялся; запуск suites не подтверждает hardware behavior.
- **Следующий шаг:** отдельно выполнить S2-39k; hardware acceptance требует собственной физической среды и evidence. Не трактовать cross-target type-check как runtime или hardware acceptance.
- **S2-30cl:** `D9 F8` is covered by the native x87-deny admission matrix. Its Linux x86_64 denial regression passed in `native_v2::tests`; execution of the x87 instruction itself and physical-hardware acceptance remain `not_run`. См. [S2-30cl report](reports/S2_30CL_X87_FPREM.md).
- **S2-30cm:** `D9 F5` is covered by the same native x87-deny admission matrix. Its Linux x86_64 denial regression passed in `native_v2::tests`; execution of the x87 instruction itself and physical-hardware acceptance remain `not_run`. См. [S2-30cm report](reports/S2_30CM_X87_FPREM1.md).
- **S2-37dj:** прежнее cross-target compile препятствие устранено. Тест denial для `ADDSD` m64 проходит Linux target type-check, но на Darwin arm64 host не запускался. См. [S2-37dj report](reports/S2_37DJ_ADDSD_MEMORY.md).

### ISSUE-028 — Неподтверждённый внешний report о CVTPI2PS memory x87 transition

- **Дата обнаружения:** 2026-10-31.
- **Компонент:** `od-isa` S2-37bg, bounded Long64 `CVTPI2PS XMM0,[RAX]` profile `s2-long-cvtpi2ps-memory-rax-v1`.
- **Статус:** открытое непроверенное сообщение о возможном расхождении; подтверждённый дефект не установлен.
- **Суть и последствия:** Intel SDM Rev. 093 Vol. 2A p. 3-224 предписывает для `CVTPI2PS` безусловно установить x87 TOP в 0 и все теги в valid после выполнения инструкции; bounded profile следует этому требованию и при memory source. Внешний QEMU issue [#4394](https://gitlab.com/qemu-project/qemu/-/work_items/4394), открытый Simon Scherer 1 сентября 2026 года, содержит standalone C/asm reproducer и сообщает о несовпадающих TOP/FTW наблюдениях при native и QEMU execution. Автор не указал CPU или hypervisor provenance; наше независимое physical reproduction не выполнялось. Поэтому аппаратное поведение и применимость наблюдения остаются непроверенными, а подтверждённый дефект не установлен.
- **Воспроизведение/основание:** сравнить правило Intel SDM с C/asm reproducer и заявленными native/QEMU TOP/FTW результатами в QEMU #4394. Локальные тесты проверяют spec-derived переход TOP=0/tag_word=0; они не заменяют physical CPU/hypervisor reproduction.
- **Следующий шаг:** при физической acceptance выполнить минимальную проверку memory-source CVTPI2PS на документированном Intel CPU и сохранить CPU/microcode, hypervisor/bare-metal, режим, точные байты и исходный/итоговый TOP/tag state. До получения evidence сохранять SDM-derived локальную семантику и не заявлять аппаратное подтверждение.

### ISSUE-022 — S2-19j address-size-32 REP LODS использовал RCX вместо ECX

- **Дата обнаружения:** 2026-10-27.
- **Компонент:** `od-isa` bounded Long64 `REP LODS` с address-size 32 (S2-19j).
- **Статус:** решена 27 октября 2026 года.
- **Суть и последствия:** цикл использовал полный RCX для проверки count и decrement. При `67` архитектурный count register — ECX: высокие 32 бита не должны запускать повторение; zero ECX выполняет ноль итераций и сохраняет RCX, а decrement после успешной итерации записывает zero-extended ECX в RCX. Старое поведение могло читать память при ECX=0, ошибочно продолжать цикл из-за высоких битов и оставлять неверный остаток после итерации.
- **Основание:** Intel SDM REP pseudocode выбирает ECX для address-size 32 и исполняет decrement только внутри `WHILE CountReg != 0`; локальные unit и literal schema-v2 cases проверяют zero count, успешный decrement, resume и no-commit memory fault. См. [CPU contract](specs/CPU_CONTRACT.md#local-s2-19j-bounded-rep-lods-address-size-32-contract), [plan](plans/STAGE_2.md#s2-19j-bounded-rep-lods-с-address-size-32), [report](reports/STAGE_2_REPORT.md#s2-19j--bounded-rep-lods-address-size-32) и [fixture notes](../tests/fixtures/x86/stage2/README.md#bounded-rep-lods-address-size-32-fixtures-s2-19j).
- **Результат:** исправлен только `Operation::RepLods`; address-size-64 и legacy REP LODS семантика не менялась. Целевые и workspace проверки, offline all-target check, formatting, fixture freshness, diff check и Graphify update подтверждены в S2-19j correction report. Hardware, native/JIT, Mac mini и SSH проверки не выполнялись.
- **Следующий шаг:** отсутствует для локальной семантической ошибки; физическая/native acceptance остаётся в общем финальном substage и не является доказательством, полученным этой правкой.

<a id="issue-042"></a>

### ISSUE-042 — layout checker worktree metadata acceptance pending

- **Дата обнаружения:** 2026-10-27; проверено в linked worktree 2026-11-04.
- **Компонент:** `tools/check_layout.py`, Git worktree root.
- **Статус:** поддержка Git metadata pointer подтверждена; оставшиеся две broken links относятся к отдельной открытой ISSUE-043.
- **Суть и последствия:** в linked worktree `.git` является обычным текстовым pointer-файлом на основную Git directory, а не каталогом или исходным файлом проекта. До исправления `python3 tools/check_layout.py` сообщал `ERROR: Unmapped root file: .git`, поэтому корректный worktree не проходил структуру. Commit `f1670fc` обновил `tools/check_layout.py` для распознавания корректного Git metadata pointer.
- **Основание и результат:** `python3 tools/check_layout.py` выполнен в `/Users/senya/.codex/worktrees/stage3-baseline/open_desktop`. Ошибка для `.git` больше не возникает; проверка обнаружила только две отсутствующие Stage 2 oracle manifests, зарегистрированные в ISSUE-043.
- **Следующий шаг:** отсутствует для Git metadata pointer handling. Восстановление двух Stage 2 manifests отслеживается отдельно в ISSUE-043.

<a id="issue-043"></a>

### ISSUE-043 — Stage 2 report ссылается на отсутствующие ignored oracle manifests

- **Дата обнаружения:** 2026-10-27.
- **Компонент:** `docs/reports/STAGE_2_REPORT.md` и локальные evidence paths под `work/`.
- **Статус:** открыта.
- **Суть и последствия:** два исторических manifest URL в отчёте Stage 2 указывают на отсутствующие `work/stage2-oracle/stage2-20261022-230853-3684f650/manifest.json` и `work/stage1-oracle/stage1-20261022-230241-3b73e810/manifest.json`; `work/**` игнорируется Git. Локально доступны Stage 1 runs с другими IDs. Эти ссылки не работают и не позволяют независимо открыть указанные сырые evidence.
- **Основание:** `python3 tools/check_layout.py` сообщил обе ссылки как broken; прямой поиск под `work/` не нашёл указанные пути, а `.gitignore` содержит `/work/**`.
- **Следующий шаг:** восстановить соответствующие архивные manifests с сохранением provenance либо обновить Stage 2 report на имеющиеся воспроизводимые evidence paths. Stage 2 файлы не менялись в текущей задаче.

<a id="issue-044"></a>

### ISSUE-044 — Непостоянный timeout generated-code fault smoke

- **Дата обнаружения:** 2026-10-29.
- **Компонент:** `tests/system/vmhost_generated_fault_smoke.py` и test-only VMHost fault helper.
- **Статус:** открыта; причина повторяющихся timeout не установлена.
- **Суть и последствия:** smoke иногда превышает свой 5-секундный deadline; в одном из прежних запусков helper затем сообщил `control frame write failed`. Два немедленных повтора в S3-38a прошли. В S3-38b один process-suite run подтвердил ожидаемый `VMHost terminated by SIGSEGV`, а последующий report-inclusive run снова достиг внутреннего timeout. Это наблюдаемая неповторяемость локального process gate, но подтверждённого продуктового дефекта пока нет.
- **Основание:** наблюдения на локальном macOS ARM64 host. Fault helper выполняет отдельный сгенерированный ARM64 stub и вызывает `od_host_abi_version`, но не вызывает `od_fragment_execute`; причина не локализована к новому C ABI block-dispatch path. В report-inclusive run `stage3-20261030-023436-6452c1b474` статус был `generated_fault_inner_timeout` после 13.961 s; сохранённые launcher stdout/stderr пусты, outer process-group cleanup завершился без оставшихся детей. Успешный run `stage3-20261030-022418-2f31426694` — отдельная сборка с другим fault-helper hash. Ранний run `stage3-20261030-021650-2286bde631` завершился harness `KeyError` до fault gate. На одном новом замороженном наборе бинарников три диагностических запуска прошли ожидаемый HELLO/READY/RUN, сгенерированный вызов завершился SIGSEGV, а launcher увидел EOF control socket и `waitpid` статус 11; прежний timeout не повторился. Сами прежние VMHost/fault-helper Mach-O имели одинаковые UUID и hashes именованных секций, но отличались hashes целых подписанных файлов и bytes в link-edit/signature. См. [S3-38a report](reports/STAGE_3_S3-38a_2026-10-29.md), [S3-38b report](reports/STAGE_3_S3-38b_2026-10-30.md) и [ISSUE-044 diagnostic report](reports/STAGE_3_ISSUE-044_DIAGNOSTIC_2026-10-30.md).
- **Следующий шаг:** причина прежнего timeout не установлена и исправление не заявляется. Не повторять попытки до нового основания; при новом timeout использовать зафиксированные opt-in phase markers для локализации. Сохранить no-retry-to-pass и 5-секундный inner deadline. Не трактовать три успешных диагностических наблюдения как исправление проблемы или доказательство стабильности S3-38b process suite.

<a id="issue-045"></a>

### ISSUE-045 — S3-20eg retry assertions

- **Дата обнаружения:** 2026-11-02.
- **Компонент:** `crates/od-host/src/lib.rs`, `s3_20eg_addr32_rsp_dword_store_uses_zero_extended_esp` (bounded S3-20eg test coverage).
- **Статус:** решена для добавленных retry assertions; точный S3-20eg host gate passed 1/1; runtime defect не установлен.
- **Суть и последствия:** the previously reviewed test asserted that injected backend write errors and entry STOP/budget exits leave CPU state and guest memory unchanged, but omitted successful retries after those outcomes. Runtime follow-up commit `525e310` adds the missing retry assertions for backend errors and entry guards. The exact updated host gate now passes, establishing the retry assertions in this bounded fixture. The successful callback-STOP path also verifies `EntryRipMismatch` with no second write. This was a test-evidence gap, not evidence of broken runtime behavior.
- **Основание:** review of the S3-20eg host test at [`od-host/src/lib.rs`](../crates/od-host/src/lib.rs) and the [S3-20eg report](reports/STAGE_3_S3-20eg_2026-11-02.md).
- **Результат:** `cargo test -p od-host --lib tests::s3_20eg_addr32_rsp_dword_store_uses_zero_extended_esp --offline -- --exact --test-threads=1` passed 1/1; the S3-34br fail-closed harness self-test passed. This closes ISSUE-045's retry-assertion evidence only. It does not establish physical-x86/Mini, owner/MMU, or full Stage 3 acceptance. The latest SSH attempt to `senya@192.168.1.14:22` timed out before remote commands; M1 remains deferred.

<a id="issue-046"></a>

### ISSUE-046 — S3-34dn artifact protection self-check

- **Дата обнаружения:** 2026-11-03.
- **Компонент:** `tests/cpu/stage3_dword_store_corpus.rs`, S3-34dn persisted mismatch artifact and replay validation.
- **Статус:** решена для create-only/version/RIP self-check acceptance; независимое Sol/high review прошло. Это не подтверждает JIT/native parity или replay реального mismatch artifact.
- **Суть и последствия:** the S3-34dn self-check attempts a create-only write over a sentinel file and confirms `AlreadyExists` while preserving its contents; it also mutates version and RIP fields and confirms parser rejection. These safeguards are now execution evidence for the labeled synthetic self-check. No runtime defect is established.
- **Основание и результат:** commit `ee2af23` updates `s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed` in [`stage3_dword_store_corpus.rs`](../tests/cpu/stage3_dword_store_corpus.rs). The exact Rust self-check and fail-closed harness self-test each passed 1/1; see the [S3-34dn report](reports/STAGE_3_S3-34dn_2026-11-03.md). The optional saved-artifact replay and S3-20eg acceptance tracked by ISSUE-045 remain separate.
- **Следующий шаг:** no further work is required for this self-check issue. Full S3-34.2 runtime/replay acceptance remains open; ISSUE-045 is separately resolved for the S3-20eg retry assertions.

<a id="issue-047"></a>

### ISSUE-047 — Rust parser rejected the addr32 checked-load predicate

- **Дата обнаружения:** 2026-11-03.
- **Компонент:** `crates/od-jit/src/ir.rs`, `is_checked_dword_load_address` / `address32_profile`.
- **Статус:** решена в текущем follow-up: добавлена закрывающая скобка, сохраняющая все прежние OR branches под guard `operand.address_size == Dword`; independent Sol/high review passed; targeted rustfmt passed. См. [repair report](reports/STAGE_3_ISSUE-047_2026-11-03.md).
- **Суть и последствия:** до исправления Rust не мог разобрать `address32_profile`: `cargo fmt --all` и `cargo check` завершались с `mismatched closing delimiter: '}'` около `ir.rs:1122`. Формы, predicate atoms и итоговые size/index/scale checks не менялись; code review подтвердил, что допуск не расширен и не сужен.
- **Основание:** исходный parser diagnostic; после правки `rustfmt --edition 2024 --check crates/od-jit/src/ir.rs` прошёл. Затронутые downstream Type/acceptance issues зафиксированы отдельно в ISSUE-048 и ISSUE-049; parser fix сам по себе их не закрывает.
- **Результат:** точечный формат прошёл; следующий `cargo check` дошёл до od-host и выявил ISSUE-048. `cargo test -p od-jit --lib --offline` запустился и дал 380 passed / 17 failed (ISSUE-049). Это подтверждает снятие parser barrier, но не завершение workspace acceptance.

<a id="issue-048"></a>

### ISSUE-048 — od-host workspace check reports nine compile errors

- **Дата обнаружения:** 2026-11-03.
- **Компонент:** `crates/od-host/src/lib.rs`, `crates/od-host/src/session.rs` and host test implementations.
- **Статус:** решена локальными compile repairs в `crates/od-host/src/lib.rs` и `crates/od-host/src/session.rs`; independent Sol/high review passed.
- **Суть и последствия:** исправлены import paths, две test-only `GuestMemory` реализации, четыре неоднозначных integer literals и сравнение error types. Семантика runtime не менялась; unsupported test memory operations возвращают явную backend error.
- **Основание и результат:** `cargo check -p od-jit -p od-host --all-targets --offline` завершился успешно. Полный `cargo test -p od-jit --lib --offline` прошёл: 397 passed / 0 failed. Точный host regression S3-18f и host regressions S3-20fk/S3-20fx прошли после исправления stale test expectations/setup. Это закрывает compile barrier и перечисленные local cases, но не заменяет stage-wide или hardware acceptance; Mini/M1 deferred.

<a id="issue-049"></a>

### ISSUE-049 — seventeen existing od-jit unit gates fail after parser repair

- **Дата обнаружения:** 2026-11-03.
- **Компонент:** `crates/od-jit/src/block.rs`, `lowering.rs`, `memory_dispatch.rs` и `memory_load_v2.rs`: admitted profiles и unit gates.
- **Статус:** решена в текущем follow-up; независимое Sol/high review исправлений прошло. Исход до ISSUE-047 неизвестен: прежний predicate не разбирался компилятором.
- **Суть и последствия:** первый запуск `cargo test -p od-jit --lib --offline` после parser repair дал 380 passed / 17 failed. Четыре ошибки относились к StoreThenStore lowering/dispatch, exact sites и PC maps; двенадцать — к admission и source expectations для addr32/qword dword load/store; одна — к fixture мутации source byte. Exact first-site builder arm для StoreThenStore добавлен в `48b554d`; расширены точные literal gates для отсутствовавших адресных форм и исправлена устаревшая negative fixture. Не установлено, существовали ли эти ошибки до ISSUE-047.
- **Результат:** четыре StoreThenStore gates прошли отдельно после исправления; затем полный `cargo test -p od-jit --lib --offline` прошёл: 397 passed / 0 failed. Это результат локального od-jit suite, без утверждения о host runtime или Stage 3 acceptance. Границы StoreThenStore описаны в [S3-26f report](reports/STAGE_3_S3-26f_2026-11-03.md).

<a id="issue-050"></a>

### ISSUE-050 — Stage 3 local harness self-tests exceed their bounded timeout

- **Дата обнаружения:** 2026-11-03.
- **Компонент:** `tests/cpu/stage3_harness.py`, mandatory `harness_self_tests` step and `tests/cpu/test_stage3_harness.py` temporary fixtures.
- **Статус:** в работе; timeout raised to 600 seconds, test fixtures moved below ignored `work/stage3-harness/`, and primary failure preservation now has a focused regression test. Full local-suite rerun remains unverified.
- **Суть и последствия:** `python3 tests/cpu/stage3_harness.py --suite local` stopped before the selected local suites because its required self-test step had a 120-second timeout. The 2026-11-03 summary (`stage3-20261103-123052-8fa49da8df`) records `returncode: null`, 138.722 seconds including cleanup/fingerprint work, and output ending during the 60th self-test. Termination left a temporary `run/logs` tree at repository root, so the final source fingerprint also changed and masked the timeout as the reported failure.
- **Основание и результат:** deleting only that generated temporary tree restored the exact starting source fingerprint. A standalone self-test invocation exceeded the context tool's 300-second RPC limit without a summary. The follow-up raises the harness step timeout to 600 seconds, routes temporary test roots under ignored `work/`, and preserves the original timeout if fingerprint comparison itself fails. The focused timeout/fingerprint test, Python compilation, and diff whitespace check passed; independent Sol/high review found no remaining issue. A post-fix full-local rerun attempt exceeded a 240-second context RPC window without a final summary; its result is unconfirmed.
- **Следующий шаг:** rerun the bounded local suite in an execution session that can observe the full self-test step and final summary; confirm the new timeout and stable source fingerprint before updating S3-41 acceptance. Mini and physical-x86 acceptance remain separate.

<a id="issue-051"></a>

### ISSUE-051 — Mini jit-block remote preflight fails in od-x86-runner tests

- **Дата обнаружения:** 2026-11-03.
- **Компонент:** `tools/x86-runner` native V2 unit tests selected by the Stage 1 physical `jit-block` preflight.
- **Статус:** решена 2026-11-03; тестовые предположения были устаревшими, подтверждённого runtime-дефекта не найдено.
- **Суть и результат:** initial Mini preflight exited 101 with 571 passed and 42 failed overall; `od-x86-runner` reported 70 passed/42 failed. Test-only corrections aligned fixture conversion with each declared profile, corrected FS-load budget and RET expectations, supplied readable code permissions for the native STOSB case, replaced stale fixture names, and made x87 rejection assertions distinguish the exact allowlist fallback. Runtime behavior was unchanged. The final exact preflight passed 613 tests with 0 failures, including 112/112 `od-x86-runner` tests. The official S3-14 CLI then completed its managed Mini comparison.
- **Основание и проверка:** complete initial and final stdout/stderr, commands, and failed test names are preserved in [`S3-14 Mini evidence`](reports/data/stage3/s3-14-mini-2026-11-03/). The successful managed comparison and its limits are documented in the [S3-14 report](reports/STAGE_3_S3-14_2026-10-26.md).
- **Следующий шаг:** нет по этой записи; более широкие Stage 3 и S3-21 physical gates остаются отдельными незавершёнными условиями.

<a id="issue-052"></a>

### ISSUE-052 — Graphify incremental merge can collapse distinct semantic nodes

- **Дата обнаружения:** 2026-11-03.
- **Компонент:** локальный Graphify incremental builder, merge/deduplication semantic citations.
- **Статус:** открыта; обновление графа 2026-11-04 выполнено через `build_merge(dedup=False)` с проверкой сохранённых идентификаторов. Ограничение upstream Graphify остаётся.
- **Суть и последствия:** обычный incremental merge группирует citations только по `(source_file, label)` и может объединить разные semantic target IDs. В попытке обновления 2026-11-03 candidate потерял бы 6 сохранённых semantic IDs; community guard также увидел 687 candidate communities против 705 сохранённых labels и отказался публиковать обновление. `--force` может опубликовать граф с потерей данных.
- **Основание:** полный refresh 2026-11-03 для текущего checkout обнаружил 3358 файлов; тогда safe updater сообщил community mismatch (687 candidate communities против 705 сохранённых labels) и риск потери 6 semantic IDs до публикации. При обновлении 2026-11-04 экспорт обычного incremental merge отказался перезаписать graph из-за отсутствующих semantic IDs. Повторный `build_merge(dedup=False)` сохранил все semantic IDs неизменённых файлов: 0 потерь вне повторно извлечённых источников, 0 непроверенных semantic shrink. Граф до переиндексации этой записи содержал 24 265 узлов и 57 871 связь. Из 210 старых ID, отсутствующих в этом графе, 100 были source-less AST-узлами, оставшимися без связей после переизвлечения; остальные 110 относились к переизвлечённым источникам и были заменены или удалены по текущему содержимому. Для 13 документов с неполным первым извлечением семантические фрагменты были повторно извлечены.
- **Следующий шаг:** исправить upstream merge key так, чтобы сохранялись разные target IDs и provenance одной метки; до этого обновлять граф через `dedup=False`, проверять ID/provenance и размер графа до публикации, а `--force` применять только после такой проверки.

<a id="issue-041"></a>

### ISSUE-041 — checked memory lowering ограничен узкими V2 MOV slices

- **Дата обнаружения:** 2026-10-27.
- **Компонент:** `od-jit` checked memory bridge / generated block runtime (S3-19/S3-20).
- **Статус:** открыта; реализованы exact production V2 load `48 8B /r` и отдельный unsafe store `48 89 /r`, qword, data/base/index RAX..RBX, signed disp8/disp32, разрешённые SIB scales 1/2/4/8, RIP-relative `mod=00,r/m=101,disp32` длиной 7 и отдельный `67 48 8B/89` dword-address profile длиной 4 с `mod=00`, без SIB/index/displacement. Direct формы имеют длины 3/4/7, SIB — 4/5/8. Отдельные shared-IR Dword store slices S3-20co/S3-20cq принимают ровно `89 03` и `89 00`. Base-less, no-index, RSP/RBP/R8+, остальные prefixes/overrides и trailing bytes отвергаются; общий IR/PC-map/cache и multiblock интеграция остаются за пределами среза. S3-24a добавляет отдельный exact 23-byte V3 artifact для bounded active-native-call случая: изменение уже исполненного prefix вызывает один committed store и dirty exit до tail, с notification до освобождения typed handle. S3-24b добавляет только fixture-case перезаписи полного 8-byte immediate последнего `MOV RCX,imm64` в этом V3 artifact; S3-24c также принимает точный immediate, если его qword пересекает два соседних mappings одного backing с непрерывными backing offsets. Полная source lease, identity и точный span проверяются до допуска; backing notification вызывает redispatch до native tail. Opcode/частичные и остальные split-mapping writes остаются запрещены; это не физический SMC oracle и не полное S3-24. S3-27a отдельно проверяет точный `ArchitecturalFault` от checked write на валидном адресе: prefix committed, store RIP/state preserved, tail не исполнен, RAM/version/cache notification unchanged. S3-27c добавляет диагностический PC map только для exact V3 fixture и проверяет его lifetime через artifact drop/cache invalidation/reuse; direct V2 maps и общий host-fault recovery не реализованы. Полный S3-27 fault/fetch/PC-map coverage остаётся открытым.
- **Суть и последствия:** общий dedicated V2 compiler/emitter/site/helper family создаёт разные private load/store artifacts и compile flavors. Load использует `MemoryOperand::access_address_48` и `CheckedMemoryHelperV1` со staged commit; store вызывает один checked all-or-nothing write и допускается только через unsafe host API с private synchronous ordinary RAM preconditions. Store artifact отклоняется load API; V1 ABI и tag 6 остаются неизменными. Generic unsafe V2 store API по-прежнему запрещает executable backing. Отдельный fixture-only S3-23c path записывает только другой неактивный executable guest backing через typed store slot/tracked owner; active-source overlap, production MMU, MMIO, DMA и общий SMC остаются запрещены.
- **Основание:** compiler tests и ARM64 native tests подтверждают supported direct/SIB register/base/index/scale/mod формы, exact RIP-relative disp32 site и отдельный 32-bit address-size profile. S3-20f differential охватывает все 16 data/base пар для обоих направлений, старшие биты base, qword data и alias `MOV RAX,[EAX]`. Sparse successful access across `0xffffffff..0x100000006` proves that 32-bit address-size truncation applies only to the starting effective address, while qword operand bytes continue linearly; distinct low sentinels remain unchanged. Отдельный unmapped case проверяет fault atomicity. S3-27a инжектирует точный architectural #PF на checked write валидного адреса и сравнивает fault payload, полное CPU state и RAM/version с независимой interpreter fixture; дополнительно проверяются STOP при fault, guard no-call, helper boundary classification и malformed host outcomes. См. [S3-20f report](reports/STAGE_3_S3-20f_2026-10-27.md), [S3-27a report](reports/STAGE_3_S3-27a_2026-10-27.md), [contract](specs/JIT_CONTRACT.md#s3-19b--s3-20-узкие-production-v2-mov-loadstore-slices) и [plan](plans/STAGE_3.md#s3-19-подключить-checked-memory-bridge). Физическая проверка Mac mini отложена до финального S3-21 gate после локального Stage 3.
- **Следующий шаг:** расширять memory lowering только отдельными проверенными формами; сохранять запрет executable backing в generic unsafe private-data store API и ограничивать code writes отдельным проверенным owner path. Production writer/MMU integration, прочие future-instruction rewrites и general active SMC остаются открытыми; физическую проверку выполнить после локального Stage 3.

- **Результат S3-20cq (2026-11-01):** exact `89 00` (`MOV [RAX],EAX`) подключён к существующему shared typed IR Dword-store пути рядом с `89 03`. Проверены full-width старый RAX как адрес, low32 старого RAX как payload, ровно один четырёхбайтовый write, сохранность RAX и adjacent bytes, crossing `0xffff_ffff`, architectural/backend callback errors, noncanonical #GP без callback, entry guards и строгая provenance/source admission. Callback STOP выставляется во время единственного успешного write и возвращается после committed RIP/retirement; HelperCall/AfterHelper/commit snapshots inadmissible, helper error replay допускается только для all-or-nothing отказа до commit. См. [S3-20cq report](reports/STAGE_3_S3-20cq_2026-11-01.md); общий ISSUE-041 остаётся открытым.

<a id="issue-037"></a>

### ISSUE-037 — unresolved branch-target handoff between block builder and IR

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-jit` bounded block builder и typed IR verifier (S3-03/S3-04).
- **Статус:** закрыта для S3-05 IR handoff; unresolved branch безопасно уходит к interpreter до Jcc, а target не выдаётся за проверенную boundary. Dispatch resolution по-прежнему требуется в S3-15.
- **Суть и последствия:** builder останавливается сразу после условной ветви и не читает недостижимый хвост; будущая или внешняя цель может оставаться `Unresolved`. Теперь lowering исключает такую Jcc из IR records и направляет prefix-only Program на `NeedInterpreter` с RIP ветви; при пустом prefix возвращается `NoNativePrefix`. Verifier не принимает unresolved target как native branch. Нельзя считать target проверенным только потому, что его числовой адрес внутри code buffer, или читать хвост ради проверки после terminator.
- **Основание и проверка:** boundary handling в [`ir.rs`](../crates/od-jit/src/ir.rs), target classification в [`block.rs`](../crates/od-jit/src/block.rs), fallback lowering в [`lowering.rs`](../crates/od-jit/src/lowering.rs), а также [S3-05 contract](specs/JIT_CONTRACT.md#s3-05-decoded-effects-to-typed-ir). Targeted tests покрывают unresolved-first, unresolved-after-prefix и другие branch classes.
- **Следующий шаг:** S3-15 определяет resolution к будущему block entry без декодирования недостижимого хвоста; до этого unresolved Jcc остаётся interpreter-only.

<a id="issue-040"></a>

### ISSUE-040 — Stage 1 test expectations reject currently accepted x87 forms

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-isa` local Stage 1 test suite; x87 decode/execute admission.
- **Статус:** открыта; пять тестов падают на Stage 3 baseline commit `9a1376b` и повторно на S3-11 worktree, остальные 706 проходят.
- **Суть и последствия:** тесты `fabs_rejects_missing_or_invalid_source_and_boundaries_atomically`, `fchs_rejects_invalid_source_and_execution_boundaries_atomically`, `fst_and_fstp_sti_reject_noncanonical_specials_and_bad_infinity_environment_atomically`, `fcomi_family_decodes_only_the_32_bare_indexed_register_forms` и `fninit_accepts_only_bare_db_e3` ожидают `Unsupported`, тогда как соответствующие x87 forms принимаются текущей реализацией. Это не подтверждённый JIT flags дефект, но мешает полному зелёному `od-isa` suite и осложняет проверку Stage 3 изменений.
- **Основание:** локальный baseline harness [отчёт S3-01](reports/STAGE_3_BASELINE_2026-10-26.md#ограничения-и-следующий-шаг) зафиксировал 703 passed/5 failed; повтор S3-11 дал те же пять имен, serial run также воспроизводит их, а изменённые files не относятся к x87 decoder/execute.
- **Следующий шаг:** сверить эти ожидания с поддержанным x87 профилем Stage 2, обновить неверные тестовые ожидания или исправить admission только при доказанном semantic дефекте; затем повторить целевые и workspace проверки.

<a id="issue-039"></a>

### ISSUE-039 — Ограниченное покрытие liveness allocator tests

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-jit::allocator` (S3-09).
- **Статус:** частично устранена в S3-30a по покрытию def/use/helper-boundary: table-driven matrix проверяет budgets 0..=4, одновременные live register homes и spills, capture-slot reuse, а также границы до `Constant`, `ReadGpr` и `CheckedLoad` definitions. S3-30p3 отдельно закрыла sum-only limitation native helper-clobber probe, проверяя индивидуально все шесть W64 values после двух вызовов. Ранее добавленные S3-10/S3-11 проверки остаются. Это не подтверждает все allocator paths, pending-flags combinations или runtime correctness вне проверенных fixtures; подтверждённого runtime-дефекта нет.
- **Суть и последствия:** allocator опирается на first/last-use в linear IR; непокрытое сочетание может ошибочно посчитать временное значение мёртвым или сохранить неверный набор register homes. Подтверждённого runtime-дефекта нет.
- **Основание:** независимое ревью S3-09 выявило отсутствие отдельных unit vectors для перечисленных liveness сочетаний. S3-30a добавила `table_driven_definition_use_and_helper_boundary_liveness_matrix`; `cargo test -p od-jit --offline` прошёл (275/275). Native helper-clobber probe прошёл (1/1); после S3-30p3 он отдельно проверяет все шесть values после обоих вызовов, что зафиксировано в [S3-30p3 report](reports/STAGE_3_S3-30p3_2026-11-01.md). Независимое Sol/high review подтвердило ожидаемые liveness, spill/capture slots, counters и новый test evidence.
- **Следующий шаг:** при изменениях allocator добавлять проверки для конкретно изменяемых путей. Текущее покрытие не является исчерпывающей матрицей runtime allocator paths; новые комбинации добавлять в пределах конкретных Stage 3 срезов. Allocator comparison и выбор реализации остаются задачей S3-30 и требуют отдельного измерительного среза.

<a id="issue-038"></a>

### ISSUE-038 — Ограничение одной MAP_JIT region и незавершённая multi-block интеграция

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-host::PublishedCode` (S3-07), будущий block cache (S3-18).
- **Статус:** частично устранена в S3-18b/S3-18c/S3-22a/S3-23a/S3-23c/S3-23d/S3-23e/S3-23f/S3-23g/S3-24a/S3-24b/S3-25a/S3-25b; bounded host arena/cache, fixture context identity, единая fixture write transaction для interpreter/V2/control, owner notifications и region-scoped redispatch между calls, tracked V2 code store, qword writes across same- or distinct-backing adjacent mappings (maximum two segments), bounded active-prefix and exact-tail-immediate exits и synchronous V3/cache slot lifetime checks реализованы. Текущая синхронная компиляция/публикация удерживает frozen snapshot, поэтому race с owner write исключён в этом пути; live-owner candidate commit потребуется при фоновой/асинхронной компиляции или освобождении snapshot до publication. Production MMU ownership, DMA/device writers, instruction rewrites beyond the exact V3 tail-immediate fixture, active-code retirement beyond this fixture, полный lifecycle и performance acceptance остаются открытыми.
- **Результат S3-24b (2026-10-29):** точный `MOV RCX,imm64` tail immediate в одном V3 fixture artifact может быть заменён одним committed qword-store; source/backing identity и exact destination span обязательны. Dirty exit перед tail, direct/alias invalidation, свежая redispatch, STOP, stale lease, сдвинутые/частичные записи и fault/backend atomicity покрыты. Тогда же split-mapping target был отклонён; S3-24c расширяет только точные два соседних mapping одного backing. Это не физическое x86 SMC доказательство и не закрывает общий S3-24. См. [S3-24b report](reports/STAGE_3_S3-24b_2026-10-29.md).
- **Результат S3-24c (2026-11-03):** тот же exact V3 tail immediate теперь допускает destination через два соседних mappings одного backing с contiguous backing offsets. Подтверждены dirty exit, единичные write/version/notification, инвалидирование предварительно закешированного tail, fresh dispatch с новым immediate и STOP после commit. Scope fixture-only; production owner/MMU и общий SMC остаются открытыми. См. [S3-24c report](reports/STAGE_3_S3-24c_2026-11-03.md).
- **Результат S3-38a (2026-10-29):** существующий register-only S0-09 C ABI `od_fragment_execute` подключён к eager multi-block dispatcher через run-local bounded arena/cache. ABI ownership/status/STOP regressions и локальные VMHost control/process smokes прошли. Cache не разделяется между handles/requests и не хранится в `FragmentInstance`; full S3-38 и остальные пункты этой записи остаются открытыми. См. [S3-38a report](reports/STAGE_3_S3-38a_2026-10-29.md).
- **Уточнение S3-15n:** production ownership prerequisites are recorded in [JIT_CONTRACT](specs/JIT_CONTRACT.md#production-memory-owner-integration-prerequisites-s3-15n): `od-machine` must own RAM/backing, mappings, MMU permissions, code leases and committed-write batches; `od-jit` consumes fetch/decode and invalidation events for compile/cache/dispatch; `od-host` owns executable publication/release. This is a documented dependency map, not an implementation. The borrowed uncached dispatcher remains fixture-bounded, and `od-machine` has no production memory owner.
- **Суть и последствия:** Apple ограничивает процесс с Hardened Runtime и allow-jit entitlement одной `MAP_JIT` region. `od-host` отвергает вторую отдельную region как `MappingAlreadyLive` (C ABI: существующий `STATUS_BUSY`). S3-18b добавляет один bounded arena с 1..=16 fixed slots по 4096 bytes и handle-owned lifetime; S3-18c соединяет её capacity с `od-jit::dispatch::MultiEntryBlockCache`, который ограничен 16 entries, 65,536 emitted bytes и 512 KiB logical metadata, использует FIFO eviction и dropped handles перед publication. S3-22a включает caller-owned nonzero context generation/profile в cache key. S3-23a добавляет fixture owner notifications для backing content, mapping replacement/removal и execute-permission changes между synchronous dispatch calls. S3-23c добавляет one-vCPU tracked fixture path: stop-RIP/STOP/budget guards сохраняют приоритет, owner сверяет site RIP/source bytes с active-source lease, проверяет полный range/permissions/version и отвергает active-source alias overlap; после result decoding committed store меняет version и инвалидирует target backing. S3-23d направляет interpreter `GuestMemory::write`, tracked V2 store и control write fixture через общие range/permission/backing/version/active-overlap проверки и единый all-or-nothing commit; interpreter/JIT notifications применяются после возврата call, control notification — до возврата control adapter. S3-23f разрешает один qword через максимум две adjacent mappings того же backing с единым slice copy. S3-23g строит фиксированный план максимум из двух сегментов для adjacent mappings разных BackingIds, валидирует версии и permissions обоих до mutation, затем под эксклюзивной fixture транзакцией копирует оба сегмента без callback/re-entry. Если сегменты contiguous в одном backing, они coalesce в один slice copy; разные backing получают отдельные copies и по одной version increment. Fixed notification batch применяется до следующего lookup; это не подтверждает hardware atomicity или SMP. Differential fixture tests сравнивают interpreter и native V2 store по CPU/RAM/version и следующему dispatch; control-write parity и failure atomicity также проверены. S3-25a подтверждает, что cache entry/compiled metadata/published handle остаются живы через backend call и result validation, eviction/drop происходит только на safe point после возврата, STOP не освобождает resident entry, а backend error удаляет его после возврата. S3-25b проверяет capacity-2 arena: один V3 slot и один cached prefix живут одновременно, третья публикация не выселяет их, cache clear и notification освобождают только cache slot, устаревший V3 handle отвергает изменённый source, а drop V3 позволяет переиспользовать slot и затем освободить/reopen mapping lease. STOP-after-commit, fault after prefix и backend-before-handle/reopen проверены соответствующими fixture cases. Полный S3-25 остаётся открытым. Это bounded fixture path, не production memory owner: DMA/device writers, production MMU, future owner-aware candidate commit если фоновой/unfrozen compilation сможет превысить lifetime snapshot, active-code retirement, SMP reclamation и deferred retirement queue остаются вне среза. Caller по-прежнему удерживает frozen snapshot/lease; `pthread_jit_write_protect_np` действует на текущий поток, а локальный gate не координирует посторонних или параллельных JIT-пользователей процесса.
- **Основание:** Apple [JIT porting documentation](https://developer.apple.com/documentation/apple-silicon/porting-just-in-time-compilers-to-apple-silicon) и [entitlement documentation](https://developer.apple.com/documentation/BundleResources/Entitlements/com.apple.security.cs.allow-jit?changes=latest_7_7_7&language=objc); Intel SDM Vol. 3A (Order No. 253668-082US, December 2023) §9.1.3 задаёт store/control-transfer/execute последовательность ([официальный PDF](https://cdrdv2-public.intel.com/812386/253668-sdm-vol-3a.pdf)); arena/store lifecycle and fixture tests в [`od-host/src/lib.rs`](../crates/od-host/src/lib.rs); owner invalidation details описаны в [S3-23a report](reports/STAGE_3_S3-23a_2026-10-27.md), [S3-23c report](reports/STAGE_3_S3-23c_2026-10-27.md), [S3-23d report](reports/STAGE_3_S3-23d_2026-10-27.md), [S3-23e report](reports/STAGE_3_S3-23e_2026-10-27.md), [S3-23f report](reports/STAGE_3_S3-23f_2026-10-27.md), [S3-23g report](reports/STAGE_3_S3-23g_2026-10-27.md), [S3-25a report](reports/STAGE_3_S3-25a_2026-10-27.md) и [S3-25b report](reports/STAGE_3_S3-25b_2026-10-27.md).
- **Следующий шаг:** implement the `od-machine` production RAM/backing/mapping/MMU owner and private synchronous RAM capability with permission/revocation tests; connect incremental fetch/decode and every committed writer (including aliases and future device/DMA writers) to versioned owner identities and active-code dirty notifications; then integrate `od-jit` dispatch/invalidation and `od-host` publication/lease release and run the owner-backed end-to-end acceptance in [S3-15n](specs/JIT_CONTRACT.md#production-memory-owner-integration-prerequisites-s3-15n). Add candidate revalidation only if writers may mutate backing during compilation outside a frozen lease. Active-code/SMP retirement remains its own S3-23/24/25 prerequisite. Final Mac mini physical comparison and cold/warm performance measurements remain separate gates. Do not create per-block MAP_JIT mappings.

### ISSUE-018 — Специальные значения и исключения indexed FCOMI-family вне bounded профиля

- **Дата обнаружения:** 2026-10-24.
- **Компонент:** `od-isa` S2-30an, indexed DB/DF FCOMI/FUCOMI/FCOMIP/FUCOMIP.
- **Статус:** открыта; текущий bounded profile поддерживает только canonical-Normal ordered operands и не моделирует x87 exception/CR0 delivery или physical native x87 behavior.
- **Суть и последствия:** расширенные формы `i=0..7` декодируются и исполняются в явно ограниченном профиле. QNaN/SNaN comparison/arithmetic behavior, denormal/infinity arithmetic or other use outside the canonical and pseudo-denormal `FLD m80fp` DB 28 load slices S2-31aa/ad, other unsupported encodings, exception masks/sticky status, traps и hardware-visible native x87 state не подтверждены; поведение вне profile остаётся Unsupported и не должно трактоваться как полная архитектурная реализация.
- **Основание:** interpreter matrices, exact schema coverage, six literal CLI fixtures, workspace tests/checks и Linux-target compile-only check для S2-30an; см. [контракт](specs/CPU_CONTRACT.md#local-s2-30an-indexed-bounded-fcomi-family-register-forms), [план](plans/STAGE_2.md#s2-30an-bare-indexed-fcomi-family-register-forms), [отчёт](reports/STAGE_2_REPORT.md#s2-30an--indexed-fcomi-family-register-forms) и [fixtures](../tests/fixtures/x86/stage2/README.md).
- **Следующий шаг:** при расширении x87 профиля сверить unordered/special-value и exception delivery semantics по Intel SDM revision 093, добавить отдельные state/exception vectors и физическую/native acceptance; до этого сохранять narrow canonical-Normal contract и native deny.

### ISSUE-017 — Нет полного x86 LOCK ordering для общего memory backend

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa::GuestMemory`, S2-21f `LOCK XADD qword [memory], register` и S2-21h explicit `LOCK XCHG qword [memory], register`.
- **Статус:** открыта; контракт записан в [MEMORY_MODEL.md](specs/MEMORY_MODEL.md), но ещё не реализован и не принят. ISSUE-017 остаётся открытой до реализации и acceptance S6-31; shared/asynchronous memory, DMA, MMIO и SMP use нельзя считать поддержанными по одному факту наличия спецификации.
- **Суть и последствия:** `atomic_fetch_add_integer` задаёт один backend вызов, wrapping update и no-mutation-on-error, но не задаёт полного x86 LOCK ordering, alignment/split-lock policy и не проверяет capability backend. Это достаточно только при отсутствии внешних конкурентных читателей/писателей и отложенных эффектов; generic `GuestMemory` не может динамически гарантировать это precondition. Unaligned local fixture проверяет backend bytes, но не hardware `#AC`/split-lock behavior.
- **Уточнение S2-21g:** обязательный `GuestMemory::mfence` completion call покрывает только явный MFENCE для opt-in private synchronous RAM. Он не служит ordering boundary для LOCK XADD и не закрывает эту проблему; S2-21f остаётся под тем же integration precondition.
- **Уточнение S2-21h:** явный `F0 + effective REX.W + 87 /r` qword memory XCHG повторно использует один `atomic_exchange_integer` backend call и принимается только для serial interpreter execution на private synchronous `FlatMemory`. XCHG с memory уже implicitly locked по Intel SDM revision 093 Vol.2D; этот local profile не подтверждает full LOCK ordering, backend capability, SMP или hardware atomicity. Native admission остаётся закрытым.
- **Уточнение S2-22g:** valid A=0 descriptor для локального `MOV FS, AX` профиля обновляется одним `GuestMemory::write` длиной 1 по access byte после чтения/проверок. Это семантический byte-sized backend access, а не утверждение о физической ширине bus transaction. Он допускается только для private synchronous RAM при serial interpreter execution без внешних наблюдателей/писателей, DMA, shared aliases, delayed writes или MMIO. Срез полагается на общий контракт no-mutation-on-error и сам по себе не задаёт полного x86 LOCK ordering или SMP semantics.
- **Уточнение S2-22h/i:** тот же узкий A=0 byte write добавлен только для bare `MOV GS, AX` на CPL3 в отдельном local-only GDT profile; S2-22i добавил один local-only последовательный data read через загруженную GS.base. Оба сохраняют private synchronous RAM и serial-execution preconditions и не закрывают общий LOCK ordering или SMP semantics.
- **Основание:** архитектурный анализ S2-21f сопоставил [memory trait](../crates/od-isa/src/memory.rs), [архитектурный контракт памяти](ARCHITECTURE.md) и Intel SDM revision 093 LOCK/XADD semantics. `FlatMemory` тестирует local split-region behavior, но не подтверждает межпоточную, DMA, MMIO или hardware атомарность.
- **Воспроизведение/условие:** использовать S2-21f `F0 48 0F C1 /r` или S2-21h `F0 48 87 /r` через `GuestMemory`, который имеет concurrent readers/writers, shared aliases, DMA, MMIO или отложенные writes; interpreter не проверит backend capability и не обеспечивает требуемый ordering.
- **Уточнение S6-04:** контракт x86-TSO, доступов, host-кандидатов, atomicity и Rust RAM ownership теперь зафиксирован в [MEMORY_MODEL.md](specs/MEMORY_MODEL.md). Он не выбирает быструю реализацию и не снимает ограничения действующего backend.
- **Следующий шаг:** реализовать общий атомарный memory path и подтвердить его проверками/acceptance по S6-31; до этого сохранять S2-21f и S2-22g/h/i ownership preconditions, не расширять LOCK формы или descriptor-write profile на shared/asynchronous/MMIO/SMP memory и не считать контракт реализацией.

### ISSUE-015 — Разногласие SDM по NT для POPF word на CPL3/IOPL0

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-17r, long-mode word `POPF` (`66 9D`) в CPL3/IOPL0.
- **Статус:** открыта; входное значение NT=1 возвращает `Unsupported`, архитектурное поведение не заявлено.
- **Суть и последствия:** Intel SDM revision 093 Vol.2B Table 4-15 и Vol.1 §7.3.13.2 p.7-22 расходятся относительно загрузки NT bit для 16-bit POPF при CPL3/IOPL0. Чтобы не закреплять семантику на основании противоречащих нормативных разделов, реализация отклоняет до commit как source NT=1, так и начальное NT=1; NT=0 поддерживается. Это сужает принятый профиль и исключает физическое утверждение для NT=1.
- **Основание:** сверка Intel SDM order number 253667-093US, revision 093, Vol.2B Table 4-15 с Vol.1 §7.3.13.2 p.7-22 при реализации S2-17r. Literal fixture `s2_17r_popfw_updates_allowed_flags` использует TF=0 и NT=0 и не разрешает разногласие.
- **Следующий шаг:** найти авторитетную errata/уточнение Intel либо надёжное дополнительное нормативное свидетельство. До этого сохранять `Unsupported` для NT=1 и не расширять fixture/native profile.

### ISSUE-016 — Не установлен restart state MOVS после destination fault

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-18a/S2-18b/S2-18c/S2-18d, long-mode non-REP `MOVSB` (`A4`), `MOVSW` (`66 A5`), `MOVSD` (`A5`) и `MOVSQ` (`REX.W A5`) with implicit `[RSI]`/`[RDI]`; S2-18o adds non-REP address-size-32 `MOVSB/W/D/Q` (`67 A4`, `67 66 A5`, `67 A5`, `67 48 A5`). Also S2-18e bare STOSB and S2-18l address-size-32 STOSB/W/D/Q.
- **Статус:** открыта только для non-REP destination-fault snapshots; это непроверенная архитектурная граница, а не подтверждённый дефект runtime.
- **Основание и последствия:** Intel SDM revision 093, Vol.2B MOVS задаёт `DEST ← SRC`, RSI/RDI и шаг по DF (1/2/4/8), но не устанавливает полный snapshot non-REP destination `#PF` после успешного source read. S2-18o локально читает полный source operand, затем выполняет одну width-sized destination write, коммитит оба index32 только после успеха; destination fault оставляет source read в trace, но не коммитит CPU state. При этом для данного bounded `GuestMemory` write failure не меняет RAM. Это local memory-model policy under ISSUE-016, not hardware restart, MMIO rollback, or LOCK/SMP atomicity claim. STOS similarly commits its index only after write success. REP suspension is separately specified in the SDM between completed iterations and is tracked by S2-19a. Fault-only fixtures remain local and are rejected by native admission.
- **Воспроизведение/условие:** preserve existing bare MOVS `*_write_fault` fixtures from S2-18a–d and `s2_18e_stosb_write_fault`; S2-18l is covered by `stos_address32_write_errors_leave_full_state_and_memory_uncommitted`. S2-18o exercises source/destination outcomes in `movs_address32_source_and_destination_errors_commit_no_cpu_or_ram_state`, all-or-error range failure in `movs_address32_partial_destination_fault_does_not_mutate_a_prefix`, and 73 local fixture cases including contiguous cross-4-GiB writes. These host results are not physical expectations.
- **Следующий шаг:** find an unambiguous normative/errata source or obtain a reliable post-fault snapshot during the shared final Mac mini acceptance substage. Until then, keep the local commit policy and do not claim the hardware RSI/RDI/RIP state after a string destination `#PF`.

### ISSUE-014 — Не установлено состояние RSP после fault у memory-destination POP

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-17j/S2-17l, long-mode qword `POP r/m64` and word `POP r/m16` (`8F /0`) с ordinary memory destination.
- **Статус:** открыта; нормативный post-fault CPU state не подтверждён, не установленный дефект runtime.
- **Основание и последствия:** Intel SDM order number 253667-093US, revision 093, Vol.2B, `POP` pp. 4-398–4-402 описывает чтение значения со старого stack top, увеличение RSP на размер операнда до записи в memory destination и вычисление адреса destination после увеличения, если RSP используется как base. Раздел перечисляет `#PF` для memory destination, но не устанавливает, обязано ли увеличенное RSP сохраняться при fault destination store. Executor вычисляет EA на staged state после `RSP += 8` для qword и `RSP += 2` для word, публикуя staged state только после успешной записи; это локальная fault policy, не подтверждённое архитектурное postcondition. Store-`#PF` regressions намеренно проверяют exception и неизменившуюся RAM, но не сравнивают GPR/RIP; native admission отвергает fault-only fixtures.
- **Воспроизведение/условие:** `8F /0` или `66 8F /0` с читаемым значением на старом `[RSP]` и memory destination, который вызывает store `#PF`. Вызвать `step` с инъекцией architectural write fault в `FlatMemory`; проверить #PF и неизменившуюся память. CPU post-state не сравнивать до уточнения нормативного поведения.
- **Следующий шаг:** найти однозначное Intel/AMD normative или errata evidence либо получить надёжный физический x86 snapshot в общем финальном Mac mini substage. До проверки не закреплять архитектурное ожидание для RSP/RIP/GPR при destination-store fault. Успешный `[RSP]` путь использует post-increment EA согласно SDM; эта успешная семантика не затронута проблемой. Контекст реализации: [S2-17j report](reports/STAGE_2_REPORT.md#s2-17j--qword-memory-destination-pop-local), [S2-17l report](reports/STAGE_2_REPORT.md#s2-17l--16-bit-memory-destination-pop-local), [CPU contract](specs/CPU_CONTRACT.md) и [plan](plans/STAGE_2.md).

### ISSUE-013 — Не подтверждён приоритет target `#GP` и stack fault для near CALL

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-isa` S2-17c/d/e, near relative `CALL rel32` (`E8`) и near indirect `CALL r/m64` (`FF /2`) с register или ordinary memory source, включая порядок source/target/stack fault checks.
- **Статус:** открыта; непроверенный приоритет combined fault, не подтверждённый дефект.
- **Основание и последствия:** executor проверяет canonicality target перед вызовом stack helper для `E8 rel32` и `FF /2`; memory `FF /2` перед этим читает qword source. Отдельные source-read fault, noncanonical-target `#GP` и stack-fault cases проходят. Для случая, когда near CALL получил noncanonical target и одновременно invalid/noncanonical guest stack, архитектурный победивший fault не установлен. Intel SDM revision 093, Vol.2A, `CALL` p.3-125 описывает operation pseudocode через временный RIP, stack capacity, push и обновление RIP, а `#GP` для target перечисляет отдельно; эта подача сама по себе не подтверждает приоритет combined fault для direct, register-indirect или memory-indirect формы. Independent Sol-medium reviews для S2-17d/e чистые; S2-17e локально проверяет отдельные source/target/stack faults без commit; combined dual-fault fixture и expected physical snapshot пока не созданы.
- **Воспроизведение/условие:** выполнить `E8 rel32`, register-only `FF /2` или memory `FF /2` (читаемый source содержит noncanonical qword) при одновременно invalid/noncanonical guest stack. Для memory CALL текущий executor сначала читает source, затем проверяет target canonicality и stack helper; наблюдаемый порядок реализации не устанавливает нормативный expected winner.
- **Следующий шаг:** найти нормативное или errata evidence; иначе проверить возможность надёжно захватить combined-fault result для direct/register/memory indirect CALL в harness, создать dual-fault fixture с expected state и включить её в общий финальный physical Mac mini acceptance evidence. До подтверждения не менять runtime по догадке. Контекст — [S2-17e report](reports/STAGE_2_REPORT.md#s2-17e--memory-near-indirect-call-local), [CPU contract](specs/CPU_CONTRACT.md) и [plan](plans/STAGE_2.md).

### ISSUE-012 — Не подтверждена семантика operand range через верх u64

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-isa` S2-08 full-width address validation диагностического long-mode профиля; S2-34.5 legacy `MOVUPS [RAX],XMM` также сохраняет ту же явную overflow policy.
- **Статус:** открыта; непроверенная semantic boundary, не подтверждённый hardware bug.
- **Основание и последствия:** проверка Intel SDM093 Vol.3A, §§4.5.1 и 6.15, не дала однозначного нормативного ответа для многобайтного доступа, у которого endpoint переполняет u64. Поэтому реализация возвращает `MemoryError::AddressOverflow` до backend и без commit; она не выдаёт неподтверждённый #GP. Это harness/memory-boundary error, не установленная x86 exception semantics.
- **Воспроизведение:** в диагностическом long-mode профиле задать `RAX = 0xfffffffffffffff8` и исполнить legacy `MOVUPS [RAX],XMM0` (`0f 11 00`); старт canonical, endpoint 16-byte operand непредставим u64. Проверить `AddressOverflow`, сохранённый CPU state и отсутствие обращения к backend. Qword пример `RAX = 0xffffffffffffffff`, `48 8b 08` остаётся исходным воспроизведением для S2-08.
- **Следующий шаг:** найти точный нормативный источник или независимое подтверждение на физическом x86 для этого диапазона, затем закрепить architectural outcome и regression. Закрыть до полной приёмки этапа 2; локальная проверка выбранного boundary policy не заменяет hardware confirmation. Контекст — [CPU contract](specs/CPU_CONTRACT.md#исполнение-mov--s2-08) и [S2-34.5](specs/CPU_CONTRACT.md#local-profile-s2-345-s2-long-movups-mem-store-v1).

### ISSUE-011 — AMD APM 4.09 не прочитан через web fetch

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** нормативные источники CPU, S2-00; проверка AMD semantics.
- **Статус:** открыта; ограничение доступа к источнику в текущей среде.
- **Основание и последствия:** повторный `web.run.open` вернул `Failed to fetch … (404) Not Found` для [страницы 40332 revision 4.09](https://docs.amd.com/v/u/en-US/40332_4.09_APM_PUB), [официального PDF](https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/40332.pdf) и [indexed content API](https://docs.amd.com/api/khub/documents/68GKiN0gMEd6bMddsmhPwg/content). Поиск показывает metadata/выдержки, но тело manual через этот инструмент не прочитано; они не считаются проверкой нормативной семантики. Наблюдение не доказывает общую недоступность сайта или отсутствие документа.
- **Повторная ограниченная проверка:** 22 октября `web.run.open` снова вернул 404 для landing 4.09, khub content, отдельного официального Volume 3 `24594.pdf` и Volume 1 khub. Другой HTTP fetch получил для landing только SPA shell «Loading application…»; после загрузки в Chrome официальный портал показал «Content not found» с сообщением о перемещённом/удалённом документе именно по этому URL. Поиск портала `40332` результата в наблюдаемой странице не дал. Отдельный `urllib.request.urlopen` GET официального [24594.pdf](https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/24594.pdf), предназначенный для обработки bytes в памяти и `pdftotext - -`, завершился `TimeoutError: The read operation timed out` с лимитом 25 s до получения PDF. PDF bytes/hash и текст разделов не получены; search snippets не использованы вместо manual. Это разные наблюдаемые границы инструментов/URL, не вывод об универсальной недоступности AMD.
- **Следующий шаг:** получить официальный AMD PDF revision 4.09 через доступный официальный канал, проверить титул/version и необходимые разделы, сохранить provenance/hash и обновить [CPU contract](specs/CPU_CONTRACT.md) и [отчёт](reports/STAGE_2_REPORT.md). До этого новые vendor-specific решения считать непроверенными по AMD; уже реализованный GPR contract подтверждён Intel SDM.

### ISSUE-010 — graphify не извлекает символы публичного C ABI header

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** производный граф `graphify-out/`, C extractor инструмента graphify.
- **Статус:** открыта; ограничение инструмента навигации, не подтверждённый дефект C header.
- **Основание и последствия:** `graphify update .` завершился успешно, но сообщил syntax error на строке 68 `include/open_desktop/host_abi.h` и отсутствие извлечённых символов этого файла. Поэтому граф недостаточен для полного анализа связей публичного ABI. Успешный workspace/VMHost build не даёт оснований считать эту диагностику ошибкой компиляции исходника.
- **Воспроизведение:** выполнить `graphify update .` на текущем checkout. Граф после ревью содержит 1184 nodes и 2363 edges; предупреждение о header сохранилось.
- **Следующий шаг:** проверить поддержку используемых C macros/атрибутов в extractor и исправить или обновить инструмент. До этого для ABI-ревью дополнительно читать нормативный header и проверять штатным компилятором. Не менять ABI только ради устранения предупреждения graphify.

### ISSUE-009 — Дефекты валидации, сохранения результатов и reducer этапа 1

- **Дата обнаружения:** 2026-10-22.
- **Компоненты:** `od-testkit::{case,result,minimize}`, `tests/cpu/stage1_oracle.py`, `stage1_harness.py`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** oracle использовал непроверенные ID как пути и не отклонял конфликтующие ID/basename; batch терял вывод при native failure, а входы не фиксировались перед запуском. Generated corpus заменял ручные cases. Tagged JSON enums игнорировали лишние поля. Reducer принимал разовый flaky mismatch, не переносил ненулевой entry RIP и превышал лимит callback при значениях 0/1.
- **Воспроизведение и исправления:** семь находок и отдельные regression tests описаны в [ревью этапа 1](reports/STAGE_1_REVIEW_2026-10-22.md). Первые четыре Rust regressions воспроизведены до исправления. Точные входы и process outputs теперь сохраняются; валидация выполняется до использования ID в путях; reducer подтверждает каждого кандидата дважды и переносит entry.
- **Результат проверки:** 31 testkit test на обоих hosts, 6 Python regressions, local/workspace/fixture checks и VMHost smoke прошли. Объединённый физический прогон: 1010 уникальных cases × 2 повтора, 2020 native snapshots, ноль расхождений; [evidence](reports/data/stage1/review-2026-10-22.json).
- **Следующий шаг:** отсутствует для перечисленных дефектов; ограничения register-only профиля сохранены в [CPU_ORACLE](testing/CPU_ORACLE.md).

### ISSUE-008 — Нет Rust toolchain и checkout на физическом x86 host

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `tools/x86-runner`, физическая проверка S1-05/S1-06 и подтверждение Linux-сборки S1-04.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** исходное отсутствие toolchain и checkout мешало проверить Linux `setrlimit`/watchdog и запускать нативные fixtures. Владелец разрешил установить toolchain и скопировать checkout в отдельный каталог.
- **Основание:** Rust stable 1.99.1 установлен в пользовательский каталог; checkout скопирован в `~/projects/open_desktop-stage1`. На Mac mini прошли Linux workspace check, тесты целевых crates, Linux `setrlimit` regression test и сборка `od-x86-runner`. Затем S1-06 автоматически исполнил 10 cases × 2 повтора на физическом Intel CPU; подробности и доказательства находятся в [CPU_ORACLE](testing/CPU_ORACLE.md), [STAND](testing/STAND.md) и [плане этапа 1](plans/STAGE_1.md).
- **Следующий шаг:** отсутствует; устранённое условие закрыто. Ответственный: инженер runner.

### ISSUE-005 — RUN_ACK опережает готовность VMHost к STOP

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost`, запуск `FragmentWorker` и C ABI lifecycle.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** успешный RUN_ACK отправляется сразу после постановки worker в DispatchQueue. C ABI теперь сохраняет STOP, пришедший до начала исполнения, и сбрасывает флаг после завершения run, поэтому остановка не зависит от планирования worker.
- **Воспроизведение:** штатный launcher с `--hex 75fe --budget 18446744073709551615 --stop-after-ms 0`; на M3 Max в 5 из 100 запусков получен `VMHost returned status 6`. Подробности и сборка — в [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** `od_fragment_request_stop` сохраняет ранний запрос для следующего run; [control-race smoke](../tests/system/vmhost_control_race_smoke.py) прошёл 100/100 запусков немедленного STOP с reason 2. Отключение канала после RUN_ACK также проверено: VMHost остановил worker и завершил процесс.

### ISSUE-006 — Неполный control frame блокирует готовый RESULT

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost/Sources/VMHostMain.swift`, `ControlFrame.read`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** POLLIN означает наличие хотя бы части кадра; блокирующий read всего кадра переставал обслуживать completion pipe. VMHost теперь неблокирующе накапливает данные и декодирует только полные кадры, продолжая опрашивать completion.
- **Воспроизведение:** после RUN_ACK передать один байт `O` следующего header и держать socket открытым. Для конечного budget=2000000 RESULT не пришёл за 2 s, тогда как контроль без частичного кадра завершился за 0.021 s со стартом процесса. Полные входы — в [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** [control-race smoke](../tests/system/vmhost_control_race_smoke.py) подтверждает доставку RESULT при незавершённых header и payload; проверка отключения канала также проходит.

### ISSUE-007 — Launcher теряет результат при завершении до отложенного STOP

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/macos/Sources/launcher.swift`, ожидание `--stop-after-ms`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** launcher раньше спал до таймера STOP и затем безусловно отправлял запрос, теряя RESULT, если helper естественно завершился первым. Launcher теперь ожидает socket и срок STOP одновременно, затем принимает RESULT без ACK или STOP_ACK перед RESULT.
- **Воспроизведение:** `--hex 48b80100000000000000 --stop-after-ms 25` возвращает exit=1 и `control frame write failed`. Подтверждено на свежих release smoke-бинарниках; [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** точное воспроизведение с задержкой 25 ms теперь завершается reason 0; 20 запусков с нулевой задержкой завершаются корректно с reason 0 либо reason 2. Допустимые порядки описаны в [CONTROL_PROTOCOL_V0.md](../protocols/control/CONTROL_PROTOCOL_V0.md).

### ISSUE-001 — Корневые пользовательские каталоги и Finder metadata

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** структура репозитория, [карта проекта](PROJECT_LAYOUT.md).
- **Статус:** решена 22 октября 2026 года.
- **Проблема и последствия:** проверка структуры обнаруживает не описанные в нормативной карте объекты корня: файлы `.DS_Store` и каталоги `.codex/`, `.opencode/`, `graphify-out/`, `iso/`. Из-за этого `check_layout.py` завершается ошибкой, хотя часть объектов может быть локальной конфигурацией или временными данными.
- **Воспроизведение:** из корня проекта запустить `python3 tools/check_layout.py`. На 22 октября 2026 года получены `ERROR: Unmapped root file: .DS_Store` и `ERROR: Unmapped root directory` для `.codex`, `.opencode`, `graphify-out`, `iso`.
- **Результат:** в [карте проекта](PROJECT_LAYOUT.md) перечислены допустимые необязательные каталоги `.codex/`, `.opencode/`, `graphify-out/` и `iso/`; layout checker учитывает их как локальные/производные данные и пропускает `.DS_Store`. Файлы и каталоги не перемещались и не удалялись. `python3 tools/check_layout.py` после изменения прошёл.

### ISSUE-002 — Задержка VMHost RUN→RESULT из-за polling worker completion

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost` control loop и VMHost completion signaling.
- **Статус:** решена 22 октября 2026 года для короткого VMHost RUN→RESULT path; длительная нагрузка и отдельный STOP latency требуют дальнейшей проверки.
- **Проблема и последствия:** первоначальный loop ожидал worker completion через `poll()` control socket с timeout 50 ms. Один короткий `MOV RAX,1` integration run дал медианный RUN→RESULT 51.33 ms [51.16–51.61] на 20 fresh VMHost processes.
- **Основание:** [S0-13 CPU baseline report](reports/STAGE_0_CPU_BASELINE_2026-10-22.md), раздел VMHost integration pilot; raw samples до/после в локальном `work/stage-0/2026-10-22-e04-vmhost-abi-ipc-resource/`. Все 40 процессов вернули ожидаемые `RAX=1`, `RIP=10`, exit reason 0 и instruction count 1. Источник timeout и реализация completion signal: `apps/vmhost/Sources/VMHostMain.swift`, `serviceStopRequests()` и `FragmentWorker`.
- **Результат:** VMHost теперь одновременно ждёт control socket и completion pipe без периодического timeout. В повторе из 20 запусков медиана RUN→RESULT составила 138.1 μs [114.5–479.7], примерно в 372 раза меньше исходного пилота. Повторные launcher и isolation smoke прошли. Изменение и ограничения измерения описаны в [S0-13 baseline report](reports/STAGE_0_CPU_BASELINE_2026-10-22.md).
- **Ограничения и следующий шаг:** сравнение было одним коротким сеансом при неконтролируемых условиях и не является изолированной мерой IPC или результатом длительной нагрузки. После исправления отдельный busy-helper pilot измерил STOP_ACK и STOP→RESULT; значения и ограничения описаны в S0-13 report. Для финального решения повторить с контролируемым фоном и длительной нагрузкой.

### ISSUE-003 — Временный SSH-доступ к M1 Pro для JIT smoke

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** внешний контрольный Mac M1 Pro, проверка S0-08/E01.
- **Статус:** решена 2026-10-22 для разрешённого smoke-прогона.
- **Суть и последствия:** исходный SSH-вход без ключа завершился отказом аутентификации, поэтому запуск на M1 Pro зависел от настройки временного ключа.
- **Основание:** по запросу владельца SSH-хост достигнут 22 октября; после явного согласия владельца на передачу трёх smoke-файлов использован временный Ed25519 ключ. Приватный ключ не попадал в репозиторий.
- **Результат:** на M1 Pro (macOS 14.6.1, MacBookPro18,3) прошли arithmetic JIT, cooperative STOP и process-isolation smoke. SHA-256 и ограничения опыта записаны в [STAND.md](testing/STAND.md); статус S0-08 — в [плане](plans/STAGE_0.md) и [отчёте](reports/STAGE_0_REPORT.md).
- **Очистка доступа:** после проверок приватный ключ на основном Mac и каталог с тремя переданными smoke-файлами на M1 удалены. Владельцу остаётся удалить строку с комментарием `open-desktop-stage0-m1-temp` из `~/.ssh/authorized_keys` на M1.

### ISSUE-004 — S0-08: Developer ID distribution и проверка конечного bundle

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-host`, VMHost supervisor и путь поставки, S0-08/E01.
- **Статус:** открыта; локальные fault-diagnostics закрыты, но этап 0 не принят до проверки пути выпуска.
- **Суть и последствия:** unit test подтверждает fault в generated code внутри дочернего процесса, а protocol-level test helper подтверждает `SIGSEGV` диагностику launcher-а. Временная ad-hoc `.app` smoke-упаковка валидна и исполняет JIT, но не подтверждает Developer ID distribution или notarization. Владелец сообщает о наличии Developer ID certificate, но соответствующая signing identity (сертификат с доступным private key) не найдена в keychains проверенных M3 Max и M1 Pro. Членство Apple Developer Program не активно; Apple notary service недоступен до его возобновления.
- **Основание:** проверки `generated_code_fault_is_confined_to_child_process`, `vmhost_generated_fault_smoke.py` и `vmhost_app_bundle_smoke.sh` прошли на M3 Max. Инвентаризация подписи и пределы теста записаны в [STAND.md](testing/STAND.md) и [отчёте этапа 0](reports/STAGE_0_REPORT.md).
- **Проверка S3-07 (2026-10-26):** на macOS 26.7.1 ARM64 ad-hoc подписанный Hardened Runtime VMHost с `com.apple.security.cs.allow-jit` прошёл launcher smoke и исполнил фрагмент; Developer ID/notarization и packaged distribution этим не подтверждены.
- **Следующий шаг:** установить на M3 Max действующую Developer ID Application identity вместе с соответствующим private key и проверить подписание конечного Open Desktop bundle. Если требуется проверить notarization, владелец должен возобновить членство Apple Developer Program и предоставить авторизацию для notary service; учетные данные не передавать через чат. Затем повторить S0-14 review. До появления доступной identity Developer ID signing не проверено, а без активного членства notarization недоступна. Ответственный: владелец конфигурации подписи совместно с инженером macOS.

### ISSUE-018 — S2-22 покрывает только ограниченные selector/descriptor slices

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` long-mode сегментная модель и S2-22.
- **Статус:** открыта; это зарегистрированный остаток этапа, не подтверждённый дефект локальных S2-22 slices.
- **Суть и последствия:** S2-22a–k задают локальные профили для отдельных FS/GS base/selector операций, CPL3 VERW, ограниченных `MOV FS, AX` / `MOV GS, AX` загрузок из GDT с A-bit update только в private synchronous RAM, точных FS/GS.base composition cases и bare null `MOV DS/ES, AX` при Long64/CPL0 с AX=0. S2-22k моделирует только видимый selector и unusable state, без descriptor lookup. Эти срезы не реализуют LDT, другие null-segment загрузки, общие accessed-bit updates, остальные GS/SS/DS/ES loads, stack checks, другие CPL/modes или применение protected-mode limit/type checks. Поэтому Stage 2 не может объявить полную сегментную архитектуру или завершить S2-22 по результатам этих профилей.
- **Основание:** ограничения зафиксированы в [плане S2-22](plans/STAGE_2.md), [CPU contract](specs/CPU_CONTRACT.md), [interpreter guide](testing/INTERPRETER.md), [fixture notes](../tests/fixtures/x86/stage2/README.md) и [отчёте этапа 2](reports/STAGE_2_REPORT.md). Локальные fixture/unit checks подтверждают только перечисленные формы.
- **Следующий шаг:** после утверждения обязательного guest profile в S2-00 выбирать отдельные bounded slices с собственным fault/state matrix и review. Не расширять текущие профили автоматически на остальные формы.

### ISSUE-019 — Семантика арифметики и инструкций x87 остаётся открытой

- **S2-29r unequal-exponent helper — bounded gap closed:** exact finite addition
  now accepts two normalized nonzero binary80 operands whose exponents differ
  by at most 62, forms an exact signed `i128` coefficient at the lower exponent,
  and rounds once through the existing packer. The unequal-only helper rejects
  equal exponents, zero operands, and gaps above 62; the composed API still
  accepts same-exponent values and treats one zero as neutral. Two zeros,
  cancellation, and finite overflow retain bounded `None` outcomes. This adds
  no FADD/FSUB instruction semantics or guest exception behavior. Review
  was clean; targeted tests, all 1,682 `od-isa` tests and workspace check passed.
  The isolated worktree layout check was unavailable for its `.git` pointer and
  absent local oracle manifests, with no layout changes. ISSUE-019 remains open
  for architectural x87 semantics and physical acceptance. See the [plan](plans/STAGE_2.md#s2-29r-exact-unequal-exponent-finite-binary80-addition-bounded-helper),
  [contract](specs/CPU_CONTRACT.md#local-s2-29r-exact-unequal-exponent-finite-binary80-addition),
  and [report](reports/S2_29R_X87_UNEQUAL_EXPONENT.md).

- **S2-29u p64-aware large-gap addition bridge — bounded helper complete:**
  `add_finite_to_binary80` now handles normalized nonzero binary80 operands with
  exponent gaps above 64 by classifying the tail below/at/above half at p=64;
  gaps through 64 keep the exact alignment path. The binary32/binary64 adapter
  retains its earlier results and diagnostics. Focused large-gap, compatibility,
  zero/cancellation and immutability tests passed; full `od-isa` passed 1,699,
  workspace all-target check, format, diff check, and Graphify update passed.
  Independent Sol/high review was clean. This is helper evidence only: it adds
  no guest x87 instruction semantics or physical x86 acceptance. ISSUE-019 stays
  open. See the [plan](plans/STAGE_2.md#s2-29u-p64-aware-large-gap-binary80-addition-bridge-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29u-p64-aware-binary80-large-gap-addition),
  and [report](reports/STAGE_2_REPORT.md#s2-29u--p64-aware-large-gap-binary80-addition-bridge).

- **S2-29v finite square-root helper — bounded helper complete:**
  `sqrt_finite_to_binary80` computes positive nonzero normalized finite binary80
  square roots with integer arithmetic and one 64-bit significand rounding.
  The exact remainder remains `u128`; literal ten-byte vectors cover all four
  rounding modes, the nearest threshold and `u64::MAX` carry, exponent
  boundaries, pseudo-denormal equivalence, and rejected zero/negative inputs.
  The relocated shared root/decomposition helpers preserve binary32/binary64
  paths, whose existing literal sqrt tests pass. Full `od-isa` tests (1706),
  component all-target check, formatting and Graphify update passed; independent
  Sol/high review was clean. This remains helper-only: no FSQRT execution, FCW,
  x87 state/status/exception semantics, or physical x86 acceptance is claimed.
  ISSUE-019 remains open for broader x87 behavior and physical acceptance. See
  the [plan](plans/STAGE_2.md#s2-29v-pure-finite-x87-square-root-to-binary80-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29v-pure-finite-x87-square-root-helper),
  and [report](reports/STAGE_2_REPORT.md#s2-29v--pure-finite-x87-square-root-helper).

- **S2-29w finite scaling helper — bounded helper complete:**
  `scale_finite_to_binary80` scales normalized nonzero finite values by an exact
  power of two and rounds once through the existing coefficient packer. Literal
  coverage checks signs and all mathematical modes, underflow and signed zero,
  subnormal/normal carry, pseudo-denormal canonicalization, exponent-add
  overflow, finite-range rejection, invalid inputs, and immutability. All 1,712
  `od-isa` tests, workspace all-target check, formatting, and independent
  Sol/high review passed. No FSCALE/FCW/FSW or guest exception behavior and no
  physical x86 acceptance are claimed. ISSUE-019 remains open; this helper did
  not establish a new issue. See the [plan](plans/STAGE_2.md#s2-29w-exact-finite-binary80-scaling-to-one-rounded-result-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29w-exact-finite-binary80-scaling-to-one-rounded-result),
  and [report](reports/STAGE_2_REPORT.md#s2-29w--exact-finite-binary80-scaling-to-one-rounded-result).

- **S2-29x exact truncated finite remainder — bounded helper complete:**
  `remainder_finite_trunc` returns an exact coefficient decomposition of
  `lhs - trunc(lhs / rhs) * rhs` for normalized nonzero finite operands and
  exponent gaps through 64. A smaller left operand is returned unchanged;
  larger gaps and malformed inputs are rejected. Literal coverage includes
  signed operands, exact zero, exponent-gap boundaries, normal/subnormal and
  pseudo-denormal inputs, `i32` exponent endpoints, and literal binary80
  composition. Focused tests passed 3/3, full `od-isa` passed 1,717 tests, and
  component/workspace checks, formatting, Graphify update, and independent
  Sol/high review passed. This helper does not implement FPREM/FPREM1, partial
  reduction/C2, quotient bits, FCW/FSW, guest exceptions, architectural
  signed-zero behavior, or physical x86 acceptance. ISSUE-019 remains open for
  architectural x87 behavior and physical acceptance. See the [plan](plans/STAGE_2.md#s2-29x-exact-truncated-finite-binary80-remainder-decomposition-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29x-exact-truncated-finite-binary80-remainder-decomposition),
  and [report](reports/STAGE_2_REPORT.md#s2-29x--exact-truncated-finite-binary80-remainder-decomposition).

- **S2-29y exact nearest-even finite remainder — bounded helper complete:**
  `remainder_finite_nearest_even` computes the exact nearest-integer-ties-even
  remainder coefficient for normalized nonzero Normal, Subnormal, and
  PseudoDenormal inputs, with a special d=-1 half-way case and `u128`
  quotient/remainder arithmetic for gaps through 64. Focused tests passed 4/4,
  all 1,724 `od-isa` tests passed, and formatting, component/workspace checks,
  and independent Sol/high review passed. This adds no FPREM1 or other ISA
  behavior, quotient condition flags, FCW/FSW effects, guest exceptions,
  architectural signed-zero choice, or physical x86 acceptance. ISSUE-019
  remains open. See the [plan](plans/STAGE_2.md#s2-29y-exact-finite-nearest-even-binary80-remainder-decomposition-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29y-exact-finite-nearest-even-binary80-remainder),
  and [report](reports/STAGE_2_REPORT.md#s2-29y--exact-finite-nearest-even-binary80-remainder-helper).

- **S2-29z finite remainder quotient metadata — bounded helper complete:** the
  truncating and nearest-even metadata APIs return the existing exact remainder
  plus unsigned `u128` quotient magnitude and separate sign; zero quotient is
  positive, and nearest-even reports the ties-to-even rounded integer quotient.
  Nearest-even preserves all negative gaps: `d <= -2` passes lhs through with
  quotient zero; `d == -1` returns zero for `A <= B` and one for `A > B`. The
  legacy helpers project these results and preserve their acceptance and
  rejection domains. Literal vectors cover values above `u64`, signed operands,
  zero quotient, exact multiples, odd/even ties, gaps through the accepted
  boundary, malformed/provenance rejects, `i32` exponent endpoints, and wrapper
  projection equivalence. The 13 focused remainder tests, all 1,741 `od-isa`
  tests, format check, component/workspace checks, and Graphify update passed.
  Layout check remains unavailable in this worktree for the pre-existing
  ISSUE-023 `.git` pointer and missing ignored oracle manifests. The initial
  Sol/high review's P2 negative-gap wording mismatch was corrected in the
  contract/report/helper docs; follow-up review is requested separately. This
  adds no FPREM/FPREM1, quotient flags,
  FCW/FSW or guest exception behavior, decoder/execution/native/JIT/C ABI
  admission, physical x86 acceptance, or performance claim. ISSUE-019 remains
  open. See [plan](plans/STAGE_2.md#s2-29z-exact-quotient-metadata-for-bounded-finite-binary80-remainders),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29z-exact-quotient-metadata-for-bounded-finite-remainders),
  and [report](reports/STAGE_2_REPORT.md#s2-29z--exact-quotient-metadata-for-bounded-finite-binary80-remainders).

- **S2-29aa direct square-root precision rounding — bounded helper complete:**
  `sqrt_finite_to_binary80_precision` rounds positive normalized finite
  binary80 square roots directly to p=24/53/64 across the binary80 exponent
  range. The legacy helper remains the p64 projection with unchanged input
  acceptance/rejection; lower precision uses exact `(E,q,R)` decomposition,
  target-bit/discarded-tail classification, and the existing normal packer.
  Independent literal bytes cover all modes and precisions, p24 even/odd exact
  midpoint ties, adjacent p24/p53 boundaries without an exact p53 midpoint
  claim, exponent carry, minimum-subnormal/max-finite/negative-exponent inputs,
  pseudo-denormal equivalence, wrapper/domain equivalence, invalid precisions,
  immutability, and a direct-rounding versus double-rounding difference.
  Local validation and review status are recorded in the [standalone report](reports/S2_29AA_X87_SQRT_PRECISION.md).
  No FSQRT execution, FCW/x87 architectural state or exceptions, native/JIT,
  physical x86 acceptance, or performance claim is established. ISSUE-019 stays
  open for broader x87 semantics and physical acceptance.

- **S2-29ab finite square-root precision diagnostics — bounded helper complete:**
  `sqrt_finite_to_binary80_precision_with_diagnostics` returns the raw binary80
  value plus mathematical inexactness and target-precision increment metadata
  at p=24/53/64. Inexactness uses `R != 0` at p64 and `L != 0 || R != 0` at
  lower precision, so exact p24 midpoint roots with `R=0` remain inexact.
  `incremented` is the pre-carry increase of the truncated positive
  significand, not architectural C1; neither field represents x87 PE/C1.
  Existing bit-only APIs remain projections with their old results and `None`
  domains. The independent literals and validation/review record are in the
  [standalone report](reports/S2_29AB_X87_SQRT_DIAGNOSTICS.md). The helper adds
  no FCW/FSW, exception or instruction behavior, native/JIT/C ABI admission,
  physical x86 claim, or generic FP diagnostics framework. ISSUE-019 remains
  open for broader x87 behavior and physical acceptance.

- **S2-29ac finite division precision diagnostics — bounded helper complete:**
  direct target-precision finite division returns mathematical inexact and
  pre-carry magnitude-increment metadata for p24/53/64. It accepts only normal
  finite binary80 results, preserves the old helper's broader subnormal domain,
  and adds no guest instruction/status behavior. Independent literal tests,
  full `od-isa`, workspace check, formatting, Graphify and clean Sol/high review
  are recorded in [S2-29ac report](reports/S2_29AC_X87_DIVISION_PRECISION.md).
  ISSUE-019 remains open.

- **S2-29ad finite multiplication precision diagnostics — bounded helper complete:**
  direct full-product rounding returns mathematical inexact and pre-carry
  magnitude-increment metadata at p24/53/64 for both signs. The new API accepts
  only normal finite results; old `multiply_finite_to_binary80` and its
  subnormal-result domain remain unchanged. Independent literal tests, full
  `od-isa`, workspace check, formatting, Graphify and clean Sol/high review are
  recorded in [S2-29ad report](reports/S2_29AD_X87_MULTIPLICATION_PRECISION.md).
  ISSUE-019 remains open.

- **S2-29ae finite addition precision diagnostics — bounded helper complete:**
  exact finite binary80 addition rounds directly to p24/53/64 for gaps through
  64, with mathematical inexact and pre-carry magnitude-increment metadata.
  Exact cancellation and non-Normal outputs return `None`; the old addition
  helper retains its zero-neutral, subnormal and larger-gap domain. Full
  `od-isa`, workspace checks and independent Sol/high review passed. A first
  subnormal-output reject vector was corrected after review to use normalized
  Normal inputs; both the new helper rejection and old-helper min-subnormal
  result now pass. See [S2-29ae report](reports/S2_29AE_X87_ADDITION_PRECISION.md).
  The initial gap≤64 bound was extended in S2-29af; this entry records the
  original slice and checks. ISSUE-019 remains open.

- **S2-29af large-gap addition precision diagnostics — bounded helper complete:**
  the precision-aware finite addition API now handles gaps above 64 with the
  existing large-gap bridge. The exact-u128 path for gaps≤64 retains its old
  bytes and diagnostics. Large-gap accepted results are inexact; zero,
  cancellation, out-of-range exact exponents, rounded overflow and malformed
  inputs remain `None`. The legacy add helper's broader outputs are unchanged.
  Full `od-isa`, workspace build, formatting, Graphify and independent Sol/high
  review pass. See [S2-29af report](reports/S2_29AF_X87_LARGE_GAP_ADDITION_PRECISION.md).
  ISSUE-019 remains open.

- **FCHS/FABS stale rejection assertions — resolved:** `fabs_rejects_missing_or_invalid_source_and_boundaries_atomically` and `fchs_rejects_invalid_source_and_execution_boundaries_atomically` now use standalone SF (`FSW=0x0040`) in the rejection vectors; IE|SF (`0x0041`) remains accepted as specified by S2-30bi. The existing tests continue to assert atomic state preservation, and the forbidden-status matrix passes. Validation: focused FABS/FCHS tests and the S2-30bi forbidden-status test pass; full `cargo test -p od-isa --offline` passes 739/739, `cargo test -p od-testkit --offline` 189/189, and `cargo test -p od-jit --offline` 5/5. No CPU semantics changed. Reference: [S2-30bi CPU contract](specs/CPU_CONTRACT.md#local-s2-30bi-historical-stack-fault-status-through-bounded-fchs-fabs); [S2-30bi report](reports/STAGE_2_REPORT.md#s2-30bi--historical-sf-only-with-ie-through-bounded-fchs-fabs).
- **S2-30ci historical FCMOVNU extent:** original exact bare `DB D9` coverage is superseded by indexed S2-30cj. The indexed local Long64 interpreter admits `DB D8..DF`; it checks TOP and `(TOP+i)&7` before PF for `i>0`, checks only TOP for self-index, and copies raw bytes/tag only for PF=0. Under FCW `0x037f` and `FSW & 0x82ff == 0`, used participants must be Valid canonical Normal or Zero exact signed zero. Local evidence: 363,520 success snapshots (358,400 indexed plus 5,120 self), 161,280 invalid class/tag snapshots (150,528 indexed plus 10,752 self), 18-stream schema, literal rows, complete producer/complement snapshots and JIT denial. Native/JIT execution, x87 exception and unordered/Special semantics, and physical x86 validation remain open; no Mac mini or SSH run was made. Next step remains the shared final batched host/physical acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30cj-bounded-indexed-fcmovnu-st0-sti-db-d8df), [plan](plans/STAGE_2.md#s2-30cj-indexed-bare-fcmovnu-st0-sti-db-d8df), [report](reports/STAGE_2_REPORT.md#s2-30cj--indexed-bare-fcmovnu-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cj-indexed-bare-fcmovnu-st0-sti-db-d8df).

- **S2-30bu bounded FCMOVB (historical ST(1) extent):** exact bare `DA C1` locally preflighted both physical operands as Valid canonical Normal or Zero exact signed zero before checking CF, copied raw bytes/tag only when taken, and preserved FSW under the local `C1=0` gate. Its 51,200 success and 21,504 invalid class/tag matrices and five-stream allowlist describe the earlier ST(1)-only extent, superseded for indexed register sources by S2-30bv. Undefined-C1 behavior, x87 exception delivery, JIT/native execution and physical acceptance remain unverified. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bu-bounded-fcmovb-st0-st1-da-c1), [plan](plans/STAGE_2.md#s2-30bu-bare-fcmovb-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30bu--bounded-fcmovb-st0-st1) and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bu-bounded-fcmovb-st0-st1).
- **S2-30bv indexed FCMOVB:** exact bare `DA C0..C7` now locally preflights physical TOP plus `(TOP+i)&7` for `i>0`; self-index `i=0` validates only TOP and leaves its exact bytes/tag unchanged. A taken indexed move copies the exact source bytes/tag; gates remain FCW `0x037f`, `FSW & 0x82ff == 0`, and Valid canonical Normal or Zero exact signed zero. The 363,520 success and 161,280 invalid class/tag interpreter matrices, per-index environment/boundary checks, 19-stream schema and eight CLI fixtures are local evidence only. Undefined-C1 behavior, x87 exception delivery, JIT/native execution and physical acceptance remain unverified. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bv-bounded-indexed-fcmovb-sti-da-c0c7), [plan](plans/STAGE_2.md#s2-30bv-indexed-bare-fcmovb-st0-sti), [report](reports/STAGE_2_REPORT.md#s2-30bv--bounded-indexed-fcmovb-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bv-bounded-indexed-fcmovb-sti).
- **S2-30ca historical FCMOVNE extent:** exact bare DB C9 ST(1)-only coverage has been superseded by S2-30cb indexed DB C8..CF. Existing C9 fixtures and producer sequences remain applicable; indexed acceptance adds all source indices under the same strict FCW/FSW and canonical Normal/Zero gates. The C9-only history is not evidence for the newly indexed forms.
- **S2-30cb indexed FCMOVNE evidence:** exact bare `DB C8..CF` selects physical `(TOP+i)&7` and preflights participants before integer ZF; self-index validates TOP only. ZF=0 copies raw binary80 bytes/tag and ZF=1 preserves them. Local coverage includes 363,520 successes, 161,280 invalid class/tag rejects, per-index environment/status/boundary checks, 19 exact schema streams, eight indexed CLI fixtures, repeat and FCOMI producer cases, and FCMOVE complement checks. JIT/native execution, exception delivery and physical x86 acceptance remain open under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-30cb-bounded-indexed-bare-fcmovne-st0-sti-db-c8cf), [plan](plans/STAGE_2.md#s2-30cb-indexed-bare-fcmovne-st0-sti-db-c8cf), [report](reports/STAGE_2_REPORT.md#s2-30cb--indexed-bare-fcmovne-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cb-indexed-bare-fcmovne-st0-sti).
- **S2-30cc historical FCMOVBE extent:** the exact bare Long64 `DA D1` result was local evidence for ST(1) only; the limitation is superseded by S2-30cd. No native/JIT or physical behavior is established by that historical slice.
- **S2-30cd indexed FCMOVBE evidence:** exact bare Long64 `DA D0..D7` carries the source index in `Operation`, preflights TOP and `(TOP+i)&7` before integer CF/ZF for i>0, and preflights only TOP for i=0. Under FCW `0x037f` and `FSW & 0x82ff == 0`, each used participant must be Valid canonical Normal or Zero exact signed zero; the raw ten-byte/tag copy occurs exactly when CF or ZF is set. The 363,520 success and 645,120 invalid participant snapshots, exact twenty-stream schema, full-state indexed fixtures, self-condition rows, wrap/tag/environment checks and JIT denial are local evidence only. C1=1, x87 exception delivery, native/JIT execution and physical x86 acceptance remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-30cd-bounded-indexed-fcmovbe-st0-sti-da-d0d7), [plan](plans/STAGE_2.md#s2-30cd-indexed-bare-fcmovbe-st0-sti-da-d0d7), [report](reports/STAGE_2_REPORT.md#s2-30cd--indexed-bare-fcmovbe-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cd-indexed-bare-fcmovbe-st0-sti-da-d0d7).

- **Историческая запись S2-30cc о FABS/FCHS test expectations — resolved:** At that point, the full `od-isa` suite had 718 passes and two failures because FABS/FCHS reject vectors seeded IE|SF. Later S2-30bi explicitly admitted seeded IE|SF history, so the two vectors were changed to standalone SF (`FSW=0x0040`), which remains forbidden without IE. State-preservation assertions remain. The related FST/FSTP subnormal and FCMOV/schema neighbor expectations mentioned in the original entry were corrected in their respective follow-ups. Current validation is recorded in the resolved [FCHS/FABS entry above](#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой); no test-maintenance follow-up remains for this record.

- **S2-30bs bounded FNOP evidence:** exact bare Long64 `D9 D0` locally preserves all x87 state except FIP and retires once under FCW `0x037f` and clean `FSW & 0x80ff`; it performs no payload inspection or data access. The 4,608-snapshot matrix, nine FSW-bit rejects, invalid FCW/missing-state and atomic boundary checks, exact four-stream schema, six CLI fixtures, and JIT denial are local evidence only. C0..C3 preservation follows local policy; physical x86/native behavior and general x87 exception behavior remain unresolved. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bs-bounded-bare-fnop-d9-d0), [plan](plans/STAGE_2.md#s2-30bs-bare-fnop-with-a-bounded-environment-gate), [report](reports/STAGE_2_REPORT.md#s2-30bs--bounded-bare-fnop), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bs-bounded-fnop).

- **S2-30bq/br bounded FFREE evidence:** exact bare `DD C0..C7` marks only physical `(TOP+i)&7` Empty under FCW `0x037f` and `FSW & 0x80ff == 0`, without reading the prior tag/payload; TOP, all payloads, other tags, complete status/control words and other state are retained. The 24,576-case opaque-state matrix, retained 3,072-case ST(0) coverage, idempotence, environment/decode/boundary rejects, exact 19-stream schema allowlist and indexed composition fixtures are local evidence only. Stack exceptions are not modeled, and no JIT/native/physical x86 behavior is established. ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30br-bounded-bare-ffree-sti-dd-c0c7), [plan](plans/STAGE_2.md#s2-30br-generalize-bounded-ffree-to-all-sti-indices), [report](reports/STAGE_2_REPORT.md#s2-30br--bounded-ffree-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30br-bounded-ffree-sti).

- **S2-30bn bounded FLD1/FLDZ evidence:** bare `D9 E8`/`D9 EE` now push exact binary80 one/zero only to an Empty physical destination under FCW `0x037f` and clean `FSW & 0x80ff == 0`; any occupied destination rejects atomically. The 1,536-case interpreter matrix, fill-eight/ninth-reject and FLD1→FLDZ→FXCH→FSTP composition plus four exact schema-v2 fixtures are local evidence only. Empty-source operations, Special arithmetic, architectural stack exceptions and native/physical acceptance remain unresolved. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bn-bounded-fld1-and-fldz-constant-pushes), [plan](plans/STAGE_2.md#s2-30bn-bare-fld1-and-fldz-bounded-local-constant-push-slice), [report](reports/STAGE_2_REPORT.md#s2-30bn--bounded-fld1-and-fldz), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bn-fld1-and-fldz-constant-pushes).
- **S2-30bo bounded FLD ST(0) evidence:** exact bare `D9 C0` copies the old TOP's canonical Normal or signed Zero raw bytes/tag into the Empty physical push slot after atomic source/destination preflight. The 5,120-state interpreter matrix, rejection classes, seven-copy/eighth-reject, FLD1/FLDZ compositions, exact four-stream schema and literal fixtures are local evidence only. Empty/Special stack behavior, stack exceptions, physical acceptance and native/JIT execution remain open; do not treat these results as broader x87 support. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bo-bounded-bare-fld-st0-d9-c0), [plan](plans/STAGE_2.md#s2-30bo-bare-fld-st0-bounded-raw-copy-push-slice), [report](reports/STAGE_2_REPORT.md#s2-30bo--bounded-fld-st0), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bo-bounded-fld-st0).
- **S2-30bp bounded FLD ST(i) evidence:** exact bare `D9 C0..C6` selects physical source `(old_TOP+i)&7`, then copies its canonical Normal/signed Zero payload and tag into Empty `(old_TOP+7)&7`, atomically preflighting both. Local coverage has 35,840 matrix successes plus source/tag/environment/destination/boundary rejection and six new indexed success fixtures. C7, Empty/Special/malformed sources, stack exceptions, and JIT/native/physical behavior remain outside the slice; the shared final physical substage is still needed before any hardware claim. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bp-bounded-bare-fld-sti-d9-c0c6), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30bp--bounded-fld-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bp-bounded-fld-sti).

- **S2-30bl bounded FXCH evidence:** the exact bare `D9 C8+i` profile now accepts either Valid-tagged canonical-Normal values or Zero-tagged exact signed zero. Nonzero indices atomically exchange all ten raw bytes and physical two-bit tags; `i=0` validates TOP only and retains its payload/tag. Local coverage retains 15,360 Normal-only states and adds 32,256 mixed indexed plus 256 self-Zero states, malformed Zero/wrong-tag/environment rejection, full-state/tag assertions, schema checks, and literal fixtures. C0/C2/C3 preservation remains a deterministic local policy without a hardware-equality claim. Empty/Special participants, stack faults, physical acceptance, and broader x87 behavior remain open under ISSUE-019; JIT/native admission remains denied. See [contract](specs/CPU_CONTRACT.md#local-s2-30bl-canonical-signed-zero-through-bounded-fxch-sti), [plan](plans/STAGE_2.md#s2-30bl-admit-canonical-signed-zero-to-bounded-fxch), [report](reports/STAGE_2_REPORT.md#s2-30bl--canonical-signed-zero-through-bounded-fxch), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bl-canonical-signed-zero-fxch).
- **S2-30ao bounded FCHS evidence:** bare `D9 E0` admits only canonical Zero-tagged signed zero through the narrow sign-operation gate; the Normal-only gates for FTST/FST/FSTP remained unchanged at that historical point. S2-30ap extends that same narrow gate to FABS; S2-30aq/ar/as later supersede those FTST/FST/FSTP limits respectively. Local interpreter and project-fixture evidence does not establish native or physical behavior. ISSUE-019 remains open for final host/physical acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30ao-signed-zero-fchs-admission-d9-e0), [plan](plans/STAGE_2.md#s2-30ao-admit-canonical-signed-zero-to-bounded-fchs-d9-e0) and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ao-signed-zero-fchs).
- **S2-30ap bounded FABS evidence (historical scope):** bare `D9 E1` admits canonical Zero-tagged signed zero, clears only its sign bit and C1, preserves the Zero tag/TOP/payload and other state, records FIP and retires once. The old statements that FST and FSTP ST(i) remained Normal-only were superseded by S2-30ar and S2-30as respectively. See [contract](specs/CPU_CONTRACT.md#local-s2-30ap-signed-zero-fabs-admission-d9-e1), [plan](plans/STAGE_2.md#s2-30ap-admit-canonical-signed-zero-to-bounded-fabs-d9-e1), [report](reports/STAGE_2_REPORT.md#s2-30ap--canonical-signed-zero-through-bounded-fabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ap-signed-zero-fabs).
- **S2-30aq bounded FTST evidence (historical scope):** bare `D9 E4` additionally accepts exact Zero-tagged signed zero, compares either sign equal, and sets C3 only. The old statements that FST and FSTP ST(i) remained Normal-only were superseded by S2-30ar and S2-30as respectively. See [contract](specs/CPU_CONTRACT.md#local-s2-30aq-canonical-signed-zero-ftst-d9-e4), [plan](plans/STAGE_2.md#s2-30aq-admit-canonical-signed-zero-to-bounded-ftst-d9-e4), [report](reports/STAGE_2_REPORT.md#s2-30aq--canonical-signed-zero-through-bounded-ftst), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30aq-signed-zero-ftst).
- **S2-30at bounded FTST evidence:** bare `D9 E4` additionally accepts only Special-tagged canonical binary80 ±infinity (E=`0x7fff`, J=1, fraction=0), comparing negative as Less/C0 and positive as Greater/no condition codes; C1 is cleared and TOP/raw bytes/tags are preserved. The interpreter covers both signs × all TOP × all 16 prior condition-code states with repeat idempotence, malformed source/tag/environment rejection and execution-boundary atomicity. Two exact-sequence `project_fixture` compositions load binary32 ±infinity and observe `0x3900`/`0x3800` through FNSTSW; schema shape is unchanged. JIT/native and physical x86 behavior remain unverified, so ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30at-canonical-signed-infinity-ftst-d9-e4), [plan](plans/STAGE_2.md#s2-30at-admit-canonical-signed-infinity-to-bounded-ftst-d9-e4), [report](reports/STAGE_2_REPORT.md#s2-30at-canonical-signed-infinity-through-bounded-ftst), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30at-canonical-signed-infinity-ftst).
- **S2-30ar bare FST ST(i) signed-zero evidence (historical):** exact Zero-tagged ±0 passes the existing `0x037f` FCW and `FSW & 0x80ff == 0` gates. FST copies captured bytes and the Zero tag to `(old_TOP+i)&7` with no pop/rotation; Normal FST behavior is unchanged. Its then-current statement that FSTP remained Normal-only was superseded by S2-30as. See [contract](specs/CPU_CONTRACT.md#local-s2-30ar-canonical-signed-zero-fst-sti-dd-d0i), [plan](plans/STAGE_2.md#s2-30ar-admit-canonical-signed-zero-to-bare-fst-sti-dd-d0i), [report](reports/STAGE_2_REPORT.md#s2-30ar--canonical-signed-zero-through-fst-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ar-signed-zero-fst-sti).
- **S2-30au bounded FST/FSTP ST(i) infinity evidence:** the shared source gate now additionally admits only Special-tagged canonical signed binary80 infinity. FST copies the ten bytes/tag without TOP change; FSTP copies then empties old TOP and advances TOP. Unit and exact-sequence project-fixture coverage passed; schema, JIT and native admission were not widened. Local evidence does not establish physical x86 behavior. ISSUE-019 remains open for the combined Mac mini/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30au-canonical-signed-infinity-through-fstfstp-sti), [plan](plans/STAGE_2.md#s2-30au-admit-canonical-signed-infinity-to-bounded-fstfstp-sti), [report](reports/STAGE_2_REPORT.md#s2-30au--canonical-signed-infinity-through-fstfstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30au-canonical-signed-infinity-fstfstp-sti).
- **S2-30bb bounded FST/FSTP ST(i) subnormal evidence:** only `(Special, Subnormal)` canonical binary80 E=0/J=0/nonzero-fraction sources now pass the shared raw-copy gate. The 16,384-case opcode/sign/TOP/index/condition/fraction matrix, malformed/tag/control/boundary rejects, raw FLD m80 composition, four seeded CLI fixtures and JIT denial are covered. Physical x86/runtime acceptance is deferred to final combined S2-31 validation; this local result does not close ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-30bb-canonical-subnormal-fst-fstp-sti), [plan](plans/STAGE_2.md#s2-30bb-admit-canonical-binary80-subnormal-to-bounded-fstfstp-sti), [report](reports/STAGE_2_REPORT.md#s2-30bb-canonical-binary80-subnormal-through-bounded-fstfstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bb-canonical-binary80-subnormal-fst-fstp-sti).
- **S2-30az bounded FST/FSTP ST(i) QuietNaN evidence:** the shared register-store source gate additionally admits only `(Special, QuietNaN)` canonical binary80 values with E=`0x7fff`, J=1 and Q=1, either sign and any low-62-bit payload. FST copies bytes/tag without changing TOP or FSW; FSTP copies before emptying old TOP and advancing TOP. The 16,384-case full-state interpreter matrix, malformed/tag/environment/boundary rejects, four exact-sequence m32 QNaN fixtures, and m32/m64 direct compositions pass. Existing Normal/zero/infinity cases and the local C1 policy remain; schema, classifier and JIT/native admission are unchanged. No physical x86 behavior is established, so ISSUE-019 remains open for combined host/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30az-canonical-qnan-fst-fstp-sti), [plan](plans/STAGE_2.md#s2-30az-admit-canonical-binary80-quietnan-to-bounded-fstfstp-sti), [report](reports/STAGE_2_REPORT.md#s2-30az--canonical-binary80-quietnan-through-bounded-fstfstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30az-canonical-qnan-fst-fstp-sti).
- **S2-30av bounded FCHS/FABS evidence:** the shared narrow sign-operation gate additionally admits only `(Special, Infinity)` for canonical signed binary80 infinity in bare `D9 E0`/`D9 E1`; sign transforms, C1 clear, TOP/tag and remaining state preserve the existing local policy. Full-state coverage includes both opcodes × both signs × all TOP × all 16 prior condition-code states, repeated-operation checks, neighboring-class/tag/environment and execution-boundary rejection, plus m32/m64 load→operation→store→FNSTSW compositions. Two exact-sequence project fixtures and one wrong-tag CLI fixture pass; the schema and JIT/native gates remain unchanged. Local evidence is not physical x86 evidence; ISSUE-019 remains open for combined host/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30av-canonical-signed-infinity-fchs-fabs), [plan](plans/STAGE_2.md#s2-30av-admit-canonical-signed-infinity-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30av--canonical-signed-infinity-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30av-canonical-signed-infinity-fchsfabs).
- **S2-30aw bounded FCHS/FABS evidence:** the shared gate additionally admits only `(Special, QuietNaN)` for canonical binary80 E=`0x7fff`, J=1, Q=1, either sign, and all low-62 payload values. FCHS toggles only the sign bit; FABS clears it; Special tag, TOP, payload and other state remain preserved, C1 clears, and no exception flags are added. A 2,048-case full-state interpreter matrix covers the two operations, signs, all TOP and prior condition-code states with four payload boundaries, repeated FABS/double FCHS, malformed neighbors, wrong tags, environment and execution boundaries. Four existing-schema m32 QNaN load→operation→store→FNSTSW CLI fixtures cover both signs/operations, and direct interpreter compositions cover m32/m64; the wrong-tag CLI rejection fixture passes. JIT remains denied and schema/classifier are unchanged. Physical/native behavior is unverified; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30aw-canonical-qnan-fchs-fabs), [plan](plans/STAGE_2.md#s2-30aw-admit-canonical-binary80-quietnan-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30aw--canonical-binary80-quietnan-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30aw-canonical-binary80-quietnan-fchsfabs).
- **S2-30ax bounded FCHS/FABS evidence:** the shared gate additionally admits only canonical `(Special, SignalingNaN)` binary80 sources with E=`0x7fff`, J=1, Q=0 and a nonzero low-62-bit payload. Both signs are accepted; FCHS toggles the sign bit, FABS clears it, and both preserve Q=0, payload, tag/TOP and other state, clear C1, and add no IE/PE. The 2,048-case full-state matrix checks both operations/signs/all TOP/all prior condition codes/four payloads, double-FCHS and repeated-FABS, malformed and wrong-tag encodings, environment, and fetch/truncation/STOP/budget/fault boundaries. A raw FLD m80fp→operation→FSTP m80fp→FNSTSW AX unit composition inspects bytes after load/operation/store; four seeded bare schema-v2 fixtures check both signs/operations with budget one, and a wrong-tag CLI case rejects atomically. JIT remains denied. No m32/m64 conversion, classifier/schema change or JIT/native admission was added. Physical x86 behavior remains unverified; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30ax-canonical-snan-fchs-fabs), [plan](plans/STAGE_2.md#s2-30ax-admit-canonical-binary80-signalingnan-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30ax--canonical-binary80-signalingnan-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ax-canonical-binary80-signalingnan-fchsfabs).
- **S2-30ay bounded FCHS/FABS evidence:** the shared gate additionally admits only `(Special, Subnormal)` canonical binary80 values with E=0, J=0 and nonzero 63-bit fraction. Both signs are accepted; FCHS toggles only the sign bit, FABS clears it, and both preserve fraction, tag/TOP and other state, clear C1, and add no DE/UE/PE/IE. A 2,048-case full-state matrix covers both operations/signs/all TOP/prior condition codes/four fractions, double-FCHS and repeated-FABS, malformed neighboring classes, wrong tags, invalid/missing state and execution boundaries. A raw FLD m80fp→operation→FSTP m80fp→FNSTSW AX composition inspects bytes after load/operation/store and checks no DE; four raw-seeded budget-one schema-v2 fixtures check both signs/operations and JIT denial, while a wrong-tag CLI case rejects atomically. The classifier/schema and all other gates remain unchanged. Physical x86 behavior remains unverified; ISSUE-019 stays open for final combined acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30ay-canonical-subnormal-fchs-fabs), [plan](plans/STAGE_2.md#s2-30ay-admit-canonical-binary80-subnormal-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30ay--canonical-binary80-subnormal-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ay-canonical-binary80-subnormal-fchsfabs).

- **Нормативная неопределённость: #D при integer-store из denormal/pseudo-denormal.** Intel SDM rev.093 Vol.1 §8.2.2/Table 8-3 говорит, что pseudo-denormal при использовании как operand трактуется как denormal и вызывает DE; §8.4.1/Table 8-9 классифицирует FIST/FISTP как arithmetic instructions; §8.5.2 предписывает #D при арифметической операции с denormal operand. При этом специализированная Vol.2A FIST/FISTP entry (pp. 3-347–348) перечисляет #IS/#IA/#P, но не #D. Текущие S2-31cq/cr/cs ожидают PE без нового DE при canonical binary80 subnormal; это пока неоднозначность нормативного текста, а не подтверждённый дефект поведения.
- **Последствия:** ожидаемое появление нового DE для integer-store из canonical subnormal и pseudo-denormal не установлено. Не расширять FISTP на pseudo-denormal с PE-only предположением до разрешения противоречия; не считать текущие PE-only фикстуры аппаратным доказательством.
- **Воспроизведение/основание:** сравнить официальный Intel SDM rev.093 Vol.1 §§8.2.2, 8.4.1, 8.5.2 с Vol.2A FIST/FISTP pp. 3-347–348 и существующими S2-31cq/cr/cs fixtures. Исходники: [Vol.1](https://cdrdv2-public.intel.com/v1/dl/getContent/671436), [Vol.2A](https://cdrdv2-public.intel.com/v1/dl/getContent/671199).
- **Статус:** открыто; фактическое поведение на x86 не проверено.
- **Следующий шаг:** в едином финальном Mac mini/physical acceptance добавить отдельные inputs с DE=0: canonical binary80 subnormal и pseudo-denormal через m16/m32 FIST/FISTP и m64 FISTP, для всех допустимых widths и RC; сравнить сохранение/установку DE, PE, результат, TOP и destination. После evidence согласовать instruction-specific contract, fixtures и status S2-31cq/cr/cs. До этой проверки native evidence отсутствует.

- **S2-31dq bounded FRNDINT evidence:** Special-tagged unnormals (biased exponent 1..`0x7ffe`, J=0) now have a local masked-invalid path producing fixed negative binary80 indefinite and sticky IE, with C1 cleared. The literal seeded fixture observes IE with FNSTSW and clears it with FNCLEX. Evidence is interpreter/project-fixture only; FLD, JIT/native and physical acceptance stay unchanged. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dq-special-unnormal-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dq-special-tagged-binary80-unnormal-through-bounded-frndint), [report](reports/STAGE_2_REPORT.md#s2-31dq--special-tagged-binary80-unnormal-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31dq-frndint-unnormal-fixture). This does not resolve or change the separate FIST/FISTP #D ambiguity above.
- **S2-31dr bounded FRNDINT evidence:** exact Special-tagged binary80 pseudo-infinities (E=`0x7fff`, J=0, zero fraction, either sign) use the S2-31dq masked-invalid indefinite path in bare D9 FC. The 12,288-case interpreter matrix covers all FCW/TOP/legal historical IE/SF/sticky-flag/prior-C1 combinations, and a seeded `project_fixture` observes IE through FNSTSW and clears it with FNCLEX. E=0 zero/tag mismatch remains rejected; pseudo-NaNs were rejected at this substage and are subsequently covered by S2-31ds. FLD and JIT/native admission are unchanged. Physical x86/Mac mini acceptance remains unperformed; this evidence does not close ISSUE-019. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dr-special-pseudo-infinity-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dr-special-tagged-binary80-pseudo-infinity-through-bounded-frndint), [report](reports/STAGE_2_REPORT.md#s2-31dr--special-tagged-binary80-pseudo-infinity-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31dr-frndint-pseudo-infinity-fixture).
- **S2-31ds bounded FRNDINT evidence:** bare D9 FC accepts Special-tagged binary80 pseudo-NaNs (E=`0x7fff`, J=0, nonzero fraction) for both signs and Q=bit62 values 0/1 through the same fixed negative indefinite/sticky IE path as S2-31dq/dr; C1 clears, no current DE/PE/SF/ZE/OE/UE is added, and admitted historical flags survive. The independent 49,152-case full-state interpreter matrix and seeded Q=0/Q=1 `D9 FC DF E0 DB E2` project fixtures are recorded. Canonical J=1 SNaN quieting, FLD/classifier scope and JIT/native denial remain unchanged; the fixtures are not native evidence. The normative basis is Intel SDM revision 093 Vol.2A FRNDINT p.3-372 and Vol.1 §8.2.2 Tables 8-3/8-10 and Table 4-3. Physical x86 and general x87 behavior remain open under ISSUE-019; this work does not resolve the separate FIST/FISTP #D ambiguity. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31ds-special-pseudo-nan-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31ds-special-tagged-binary80-pseudo-nan-through-bounded-frndint), [report](reports/STAGE_2_REPORT.md#s2-31ds--special-tagged-binary80-pseudo-nans-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31ds-frndint-pseudo-nan-fixtures).
- **S2-31du bounded FLD OE evidence:** only exact bare D900/DD00 widened their local FSW mask from `0x80cc` to `0x80c4`, admitting historical OE while preserving IE/DE/OE/UE/PE histories. The 131,072-case full-state matrix includes both prior-C1 values and all 32 sticky combinations; seeded OE-only FNSTSW fixtures and same-width FST-overflow→FLD-infinity→FNSTSW/FNCLEX compositions remain local interpreter evidence. OE producers are covered only under nearest and directed-away rounding. ZE/SF/ES/B continue to reject; S2-31ak rejection fixtures now use ZE, and the schema regression verifies legal ZE-based Normal Unsupported fixtures for D900/DD00 are rejected after changing only FSW to OE-only. Native runtime, physical x86, Mac mini and general exception delivery remain unverified; this task does not resolve ISSUE-019. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31du-historical-sticky-oe-for-bounded-m32m64-fld), [plan](plans/STAGE_2.md#s2-31du-admit-historical-sticky-oe-through-exact-binary32binary64-fld), [report](reports/STAGE_2_REPORT.md#s2-31du--historical-sticky-oe-for-bounded-m32m64-fld), and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-30f evidence:** destination-index literal vectors cover varied TOP/wrap, alias with unrelated Empty tags, source preservation, bad used tags, specials, unequal exponents and inexact coefficients. DC decode is exhaustive; testkit schema accepts all eight bare encodings and native admission remains fail-closed. Full x87 architectural behavior remains unverified.
- **S2-30g evidence:** all 64 TOP/destination pairs verify physical result placement, pop/tag/TOP updates and payload preservation; i=0 Empty-tag behavior, i=1 new ST(0), wrap, exact carry, i=0 overflow rejection, unsupported/fetch/stop/budget atomicity, exhaustive DE decode, fixture-schema acceptance and native fail-closed behavior are covered. Full x87 architectural behavior remains unverified.
- **S2-30h evidence:** literal subtraction vectors cover all eight TOP values and i=1..7, source payload sentinels, sign combinations, ±2^-63, minimum Normal and maximum-coefficient carry. Full-state rejects include every i=0/TOP alias, odd 65-bit coefficient, exact subnormal output, overflow and unsupported input classes. D8 ModR/M/prefix/WAIT/truncation and neighbor forms are checked; schema and runner cover i=1..7 success/i=0 Unsupported, while native admission remains closed. Full x87 architectural behavior remains unverified.
- **S2-30i evidence:** literal destination-form vectors cover sign/order asymmetry, ±2^-63, maximum even 65-bit carry, minimum Normal, and exact placement for every TOP×i=1..7 pair. Full-state atomic rejects cover alias, missing state, FCW/FSW, tags, unequal exponents, all raw non-Normal classes, cancellation, odd 65-bit coefficient, exact subnormal output and overflow. DC ModR/M/prefix/fetch boundaries, fixture schema and runner matrix are covered; native admission remains closed. Full x87 architectural behavior remains unverified.
- **S2-30j evidence:** DE E8+i uses the shared exact FSUB result gate, writes to old physical `(TOP+i)&7`, empties old TOP, increments TOP, clears C1, records FIP and retires once. Literal vectors cover sign/order cases, ±2^-63, maximum even carry and minimum Normal; every TOP×i=1..7 checks physical placement, TOP wrap/pop, preserved payloads and state. Alias for every TOP and numeric/environment/tag/domain rejects compare full state; fetch, stop and zero-budget boundaries, exhaustive DE ModR/M/prefix decode, schema acceptance and all-eight native denial are covered. Full x87 architectural behavior remains unverified.
- **S2-30k evidence:** D8 E8+i computes ST(i)−ST(0) into old physical TOP with no pop. Literal direction/sign and exact boundary vectors, all TOP×i placements, alias/domain full-state rejection, fetch/stop/budget, exhaustive D8 decode/prefix, fixture schema/runner and native fail-closed behavior are covered. Full x87 architectural behavior remains unverified.
- **S2-30l evidence:** DC E0+i computes ST(0)−ST(i) into old physical (TOP+i)&7 without pop. Local vectors cover all TOP×i destinations, exact boundary values, alias rejection and fetch/stop/budget atomicity; decoder/schema/native-deny coverage is documented with the implementation. Full x87 architectural behavior remains unverified.
- **S2-30m evidence:** DE E0+i computes ST(0)−ST(i) into old physical (TOP+i)&7, then empties old TOP and increments TOP. Literal vectors cover every TOP×i pair, signs/order, ±2^-63, maximum accepted even carry and minimum Normal; alias and numeric/environment/tag/domain rejects preserve complete state. Fetch/truncation/STOP/budget boundaries, exhaustive DE ModR/M and prefix/REX/WAIT decoding, schema alias representation, all-index fixture-runner results and eight-code native denial are covered. Full x87 architectural behavior remains unverified.
- **S2-30n evidence:** D8 C8+i computes the exact product into old physical TOP without pop. Local vectors cover every TOP×i pair (including the valid i=0 square), sign XOR, differing exponents, inexact/overflow/subnormal rejection, raw class/environment/tag rejection, and fetch/stop/budget atomicity. D8 decoding, fixture schema, runner state checks and native fail-closed admission cover the encoding group. Full x87 architectural behavior remains unverified.
- **S2-30o evidence:** DC C8+i computes the exact product into old physical `(TOP+i)&7` without pop. Local vectors cover every TOP×i pair (including the valid i=0 square), payload sentinels, full-width-significand multiplication, and inexact/overflow/subnormal plus fetch/stop/budget atomicity. DC decoding preserves adjacent FADD/FSUBR/FSUB groups; fixture schema and native fail-closed coverage include all eight encodings. Full x87 architectural behavior remains unverified. The native admission test defect for DE E0..E7 was corrected by adding `FsubrpStSt0` to the explicit x87 deny set.
- **S2-30p evidence:** DE C8+i computes exact old `ST(i)*ST(0)` into old physical `(TOP+i)&7`, then empties old TOP and increments TOP. Literal coverage checks all TOP×i pairs, including i=0 square write-before-empty, all sign combinations, differing exponents, a full-width significand, i=0 inexact/overflow rejection, full-state inexact/range/raw-class/environment/tag rejection, and fetch/stop/budget atomicity. Exhaustive DE decoding preserves FADDP, FSUBRP and FSUBP neighbors; the fixture schema and runner cover DE C8..CF, including a no-stop/budget-one inexact DE C8 rejection, and native admission remains closed. Full x87 architectural behavior remains unverified.
- **S2-30q evidence:** D8 F0+i divides old ST(0) by ST(i) exactly and writes old physical TOP without popping. Literal coverage includes all TOP×i pairs, 15/3=5, 1/3 rejection, all signs, unequal exponents, lhs-significand<rhs normalization, full-width significand divided by one, i=0 ±minimum/±maximum Normal self-division to +1, and exact overflow/subnormal range rejection. Environment/tag/raw-class/zero/inexact rejects and missing state compare complete state; fetch/stop/budget preserve atomicity. Exhaustive D8 decode, schema acceptance of all eight encodings with an actual no-stop D8 F1 inexact case, runner coverage and explicit native denial are recorded in the S2-30q report. Full x87 architectural behavior remains unverified.
- **S2-30r evidence:** DE F8+i divides old ST(i) by old ST(0), writes to old physical `(TOP+i)&7`, empties old TOP and increments TOP. Literal vectors cover every TOP×i pair, ordered 15/3=5, the i=0 exact +1 payload before pop, sign cases, TOP wrap, and full-state atomic inexact 1/3 and 2/3, overflow, subnormal, environment, tag, raw-class and absent-state rejection. Fetch, truncation, stop and zero-budget boundaries are included. Exhaustive DE decoding preserves neighboring groups and leaves DE F0..F7 unsupported; schema and runner cover all eight successful encodings and no-stop/budget-one inexact DE F9 rejection. Native admission stays closed. Full x87 architectural behavior remains unverified.
- **S2-30s evidence:** DE F0+i divides old ST(0) by old ST(i), writes to old physical `(TOP+i)&7`, empties old TOP and increments TOP. Literal tests cover all TOP×i placements, ordered 15/3=5, all sign combinations, exponent differences, a full-width significand divided by one, positive/negative i=0 self-division producing +1 before pop, and full-state atomic inexact 1/3 and 2/3, overflow, subnormal, environment, tag, raw-class and missing-state rejection. Fetch/truncation/stop/budget boundaries, exhaustive DE decoding with neighboring groups, schema admission of all eight encodings with an inexact DE F1 no-stop/budget-one fixture, runner matrix and native denial are covered. Full x87 architectural behavior remains unverified.
- **S2-30t evidence:** D8 F8+i divides old ST(i) by old ST(0), overwrites old physical TOP without pop, clears C1, records FIP and retires once. Literal vectors cover all TOP×i placements, ordered 15/3=5, i=0 exact +1 with tags/TOP unchanged, signs, unequal exponents, full-width significand and atomic rejection of actual inexact 1/3 plus environment/tag/class/missing-state failures. D8 exhaustive ModR/M/prefix handling, schema acceptance of all eight forms with a no-stop/budget-one D8 F9 inexact case, testkit runner and native fail-closed admission are covered. Full x87 architectural behavior remains unverified.
- **S2-30v evidence:** DC F8+i divides old ST(i) by old ST(0) into old physical `(TOP+i)&7` without pop. Literal vectors cover all 64 TOP×i placements, ordered 15/3=5, the i=0 exact +1 alias, signs, unequal exponents and Normal edges, plus full-state atomic rejection of valid-tag inexact 1/3, invalid environment/tags/classes and out-of-range outputs. Exhaustive DC ModR/M/prefix checks preserve DC F0..F7 FDIVR; schema and runner cover all eight encodings plus no-stop/budget-one DC F9 rejection, and native admission denies all eight. Full x87 architectural behavior remains unverified.
- **S2-30w evidence:** DD D8+i copies old ST(0) as ten untouched raw bytes to old physical `(TOP+i)&7`, marks the destination Valid, then empties old TOP and advances TOP; the register array is not rotated. Literal vectors cover all 64 TOP×i pairs, each prior destination tag for i!=0, signed/minimum/maximum Normal values, untouched sentinels, i=0 Empty result tag and i=1 new ST(0). Invalid source tag/class, FCW/FSW and missing-state inputs reject atomically; fetch/truncation/STOP/budget boundaries preserve state. DD ModR/M/prefix tests, schema admission, all-index CLI runner checks, a no-stop/budget-one source-tag rejection fixture and all-eight native denial are covered. Full x87 architectural behavior remains unverified. See [S2-30w contract](specs/CPU_CONTRACT.md#local-s2-30w-bounded-fstp-sti-dd-d8i), [plan](plans/STAGE_2.md#s2-30w-bare-bounded-fstp-sti-local-raw-copy-slice), [report](reports/STAGE_2_REPORT.md#s2-30w--bounded-fstp-sti-local-raw-copy-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30x evidence:** DD D0+i copies ten raw bytes from old ST(0) to old physical `(TOP+i)&7`, marks the destination Valid, and preserves TOP plus the complete FSW. Literal vectors cover all 64 TOP×i pairs, every old destination tag for i!=0, signed/minimum/maximum Normal payloads, i=0 alias, and complete-state source/environment/fetch/truncation/STOP/budget rejection. Exhaustive DD ModR/M checks preserve the existing D8..DF FSTP decode; schema accepts all eight encodings and the DD D1 source-tag rejection fixture, CLI tests cover all destinations, and native admission denies all eight forms. Full x87 architectural behavior remains unverified. See [S2-30x contract](specs/CPU_CONTRACT.md#local-s2-30x-bounded-fst-sti-dd-d0i), [plan](plans/STAGE_2.md#s2-30x-bare-bounded-fst-sti-local-raw-copy-slice), [report](reports/STAGE_2_REPORT.md#s2-30x--bounded-fst-sti-local-raw-copy-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30y evidence:** Bare `D9 E0` flips only ST(0)'s raw sign bit for an existing x87 state with FCW `0x037f`, clear exception/B bits, Valid physical TOP tag, and canonical Normal source; it clears C1, records FIP, and retires once. Literal vectors cover every TOP, both signs and minimum/maximum/full-significand Normal values, exact one-bit changes, C1 clear/set, double-FCHS payload restoration, full-state environment/tag/class/missing-state rejection, and fetch/truncation/STOP/budget boundaries. Exhaustive D9 second-byte decode accepts only E0; schema and CLI cover success plus a no-stop/budget-one Empty-tag rejection, and native admission explicitly denies D9 E0. Full x87 architectural behavior remains unverified. See [S2-30y contract](specs/CPU_CONTRACT.md#local-s2-30y-bounded-fchs-d9-e0), [plan](plans/STAGE_2.md#s2-30y-bare-bounded-fchs-sign-bit-local-slice), [report](reports/STAGE_2_REPORT.md#s2-30y--bounded-fchs-sign-bit-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30z evidence:** Bare `D9 E1` clears only ST(0)'s raw sign bit for an existing x87 state with FCW `0x037f`, clear exception/B bits, Valid physical TOP tag, and canonical Normal source; it clears C1, records FIP and retires once. Literal vectors cover every TOP, both signs and minimum/maximum/full-significand Normal values, exact sign-bit-only mutation, C1 clear/set, C0/C2/C3 retention, FABS idempotence, full-state missing/environment/tag/class rejection and fetch/truncation/STOP/budget boundaries. Exhaustive D9 decode accepts only E0→FCHS and E1→FABS; schema/CLI cover success and a no-stop/budget-one Empty-tag rejection, while native admission explicitly denies FABS and D9 E1. Full x87 architectural behavior remains unverified. See [S2-30z contract](specs/CPU_CONTRACT.md#local-s2-30z-bounded-fabs-d9-e1), [plan](plans/STAGE_2.md#s2-30z-bare-bounded-fabs-sign-bit-clear-local-slice), [report](reports/STAGE_2_REPORT.md#s2-30z--bounded-fabs-sign-clear-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30aa evidence:** Bare `D9 E4` compares canonical-Normal ST(0) with zero under the bounded source gate, replacing all C0/C1/C2/C3 bits from sign, preserving TOP/tags/raw payloads and other state, recording FIP and retiring once. Literal vectors cover all TOPs, both signs, minimum/maximum/full-significand Normals, all initial condition-code combinations including all-set, FTST→FNSTSW AX composition, full-state invalid environment/tag/class/missing-state rejection and fetch/truncation/STOP/budget boundaries. Exhaustive D9 decoding accepts exactly E0→FCHS, E1→FABS and E4→FTST; two success fixtures plus a no-stop/budget-one Empty-tag rejection pass schema/CLI coverage. Native admission explicitly denies FTST. Full x87 architectural behavior remains unverified. See [S2-30aa contract](specs/CPU_CONTRACT.md#local-s2-30aa-bounded-ftst-d9-e4), [plan](plans/STAGE_2.md#s2-30aa-bare-bounded-ftst-canonical-normal-comparison-slice), [report](reports/STAGE_2_REPORT.md#s2-30aa--bounded-ftst-canonical-normal-comparison-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ab/S2-30ac evidence:** The original bare `D8 D1` bounded FCOM ST(1) slice is generalized to exactly bare `D8 D0+i` for all eight source indices. It compares old physical TOP with `(TOP+i)&7` by exact sign/exponent/64-bit-significand ordering, replaces all condition bits, preserves complete register/tag/environment payload state, records FIP and retires once; i=0 validates the shared Valid Normal alias once and returns equality. Vectors cover all 64 TOP×i placements, wrap/alias, all initial condition-code combinations, relations/sign orders, low-significand distinctions, Normal endpoints, independent tag/class/environment rejects, alias failure, sentinels, and fetch/truncation/STOP/budget atomicity. Existing S2-30ab literal fixtures remain; S2-30ac adds a literal D8 D7 success and a no-stop/budget-one invalid-ST(7)-tag fixture. Schema and CLI cover all eight indices and exact FCOM→FNSTSW AX sequences; unsupported D8 D8..DF remain rejected. Native admission now explicitly denies all eight D8 D0..D7 encodings. Full x87 architectural behavior remains unverified. See [S2-30ac contract](specs/CPU_CONTRACT.md#local-s2-30ac-bounded-fcom-sti-d8-d0i), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30ac--bounded-fcom-sti-exact-normal-compare), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ad evidence:** Bare `D8 D9` compares old physical ST(0)/ST(1) through the existing exact canonical-Normal FCOM helper, replaces the FSW condition field, empties the old physical TOP tag, advances TOP with wrap, preserves all register payloads and unrelated state, records FIP and retires once. Literal and interpreter vectors cover all TOP values and initial condition fields, relation/sign/exponent/low-significand ordering, independent lhs/rhs tag/class and environment rejects, sentinels, and fetch/truncation/STOP/budget atomicity. Schema and CLI admit D8 D9 plus exact D8 D9 DF E0 observation; the invalid rhs-tag fixture is no-stop/budget-one. Native admission explicitly denies D8 D9. Full x87 architectural behavior remains unverified. See [S2-30ad contract](specs/CPU_CONTRACT.md#local-s2-30ad-bounded-fcomp-st1-alias-d8-d9), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30ad--bounded-fcomp-st1-compare-and-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ae evidence:** Generalized the FCOMP operation to `FcompSt { source }` and accepts exactly bare `D8 D8..DF`. It compares old physical TOP with old `(TOP+i)&7` through the exact canonical-Normal helper, then replaces C0/C1/C2/C3, empties old TOP, advances TOP, preserves physical payload bytes, records FIP and retires once. `i=0` validates the shared source before equality and still pops once. Interpreter vectors cover all 64 TOP×i pairs, all initial condition fields and comparison relations, independent operand tag/class rejects, environment and execution-boundary atomicity. Eight literal success fixtures cover every i; schema/CLI admit all encodings and exact `D8 D8+i DF E0` observation with the corresponding one/two-instruction budget. Native admission explicitly denies all eight FCOMP encodings. Full x87 architectural behavior remains unverified. See [S2-30ae contract](specs/CPU_CONTRACT.md#local-s2-30ae-bounded-fcomp-sti-d8-d8i), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30ae--bounded-fcomp-sti-compare-and-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30af evidence:** Bare `DE D9` compares old physical ST(0)/ST(1) using the bounded canonical-Normal helper, then replaces the condition field, empties both old physical tags and advances TOP by two with wrap. Raw physical payloads and unrelated state stay unchanged; FIP/RIP and retirement update once. Interpreter vectors cover all TOP values, all three relations and all 16 initial condition fields, including wrap to 0/1 and an unused Empty ST(2); bad lhs/rhs and environment/fetch/truncation/STOP/budget inputs reject atomically. Schema/CLI cover exact `ded9`, `ded9dfe0`, and actual invalid-rhs no-stop/budget-one rejection. Exhaustive DE decoding retains existing arithmetic and leaves DE D8/DA..DF unsupported; native admission explicitly denies FCOMPP. Full x87 architectural behavior remains unverified. See [S2-30af contract](specs/CPU_CONTRACT.md#local-s2-30af-bounded-fcompp-de-d9), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30af--bounded-fcompp-compare-and-two-entry-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ag evidence:** Bare `DD E1` FUCOM compares old physical TOP/TOP+1 through the exact integer canonical-Normal helper, replaces condition bits with greater/less/equal, clears C1 by explicit local policy, preserves TOP/tags/raw payloads, records FIP and retires once. All TOP×relation×16 incoming condition combinations, signed/exponent/low-significand ordering, independent bad lhs/rhs tags and classes, environment and fetch/truncation/STOP/budget atomicity are covered. Exhaustive DD decode retains FST/FSTP neighbors; schema and literal runner fixtures cover `dde1`, `dde1dfe0`, and an actual invalid-rhs-tag no-stop/budget-one rejection. Native admission denies FUCOM/DD E1. FUCOM unique QNaN behavior is excluded by the canonical-Normal-only domain. Full x87 architectural behavior remains unverified. See [S2-30ag contract](specs/CPU_CONTRACT.md#local-s2-30ag-bounded-fucom-st0-st1-dd-e1), [plan](plans/STAGE_2.md#s2-30ag-bare-bounded-fucom-st0-st1-local-canonical-normal-slice), [report](reports/STAGE_2_REPORT.md#s2-30ag--bounded-fucom-st0-st1-compare-without-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ah evidence:** Bare `DD E9` FUCOMP reuses the bounded canonical-Normal ST(0)/ST(1) compare helper, replaces condition codes, clears C1 by local policy, empties only old physical TOP, advances TOP with wrap, preserves raw register payloads and old ST(1) tag, records FIP and retires once. Interpreter vectors cover all TOP×relation×16 condition combinations, payload sentinels, independent invalid operand tags/classes and environment, plus fetch/truncation/STOP/budget atomicity. Schema and literal CLI fixtures cover `dde9`, `dde9dfe0`, and actual invalid-old-ST(1)-tag no-stop/budget-one rejection; native admission denies FUCOMP/DD E9. QNaN/special-value semantics and full x87 behavior remain unverified. See [S2-30ah contract](specs/CPU_CONTRACT.md#local-s2-30ah-bounded-fucomp-st0-st1-dd-e9), [plan](plans/STAGE_2.md#s2-30ah-bare-bounded-fucomp-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30ah--bounded-fucomp-st0-st1-compare-and-one-entry-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ai evidence:** Bare `DA E9` FUCOMPP preflights both old physical canonical-Normal operands through `bounded_x87_fcom_result(state, 1)`, replaces condition bits with greater/less/equal, clears C1 by local policy, empties both old physical tags, and advances TOP by two with wrap. Payload bytes remain untouched; FIP/RIP and retirement update once. Interpreter tests cover all TOP×relation×16 condition fields, wrap, an unused Empty ST(2) with arbitrary payload, independent operand tag/class failures, environment, fetch/truncation/STOP/budget atomicity. DA decode accepts only E9; schema/CLI cover exact `dae9`, `dae9dfe0`, and actual invalid-old-physical-ST(1)-tag no-stop/budget-one rejection. Native admission explicitly denies DA E9. Intel SDM revision 093 Vol.2A pp.3-402–3-404 opcode table/prose says two pops while p.3-403 pseudocode shows one; this slice follows the explicit table/prose. General special-value, stack-fault, exception, `#NM` and native x87 semantics remain unverified. See [S2-30ai contract](specs/CPU_CONTRACT.md#local-s2-30ai-bounded-fucompp-da-e9), [plan](plans/STAGE_2.md#s2-30ai-bare-bounded-fucompp), [report](reports/STAGE_2_REPORT.md#s2-30ai--bounded-fucompp-compare-and-two-entry-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30aj evidence:** Bare `DB F1` FCOMI uses the bounded canonical-Normal compare helper after checking x87 state, FCW/FSW and both physical tags. It maps greater/less/equal to cleared relation bits/CF/ZF, defines all `0x8d5` RFLAGS bits, clears only FSW.C1, and preserves other x87 state. Coverage includes all TOP×relation×16 initial FSW condition combinations, all 64 initial affected-flag masks, invalid operand tags/classes, environment and fetch/truncation/STOP/budget atomicity, exhaustive DB decode/prefixes, exact `dbf1`/`dbf1dfe0` fixture schema/CLI vectors with actual bad old ST(1) tag, and explicit native denial. General x87 exception/special-value behavior and native execution remain unverified. See [S2-30aj contract](specs/CPU_CONTRACT.md#local-s2-30aj-bounded-fcomi-st0-st1-db-f1), [plan](plans/STAGE_2.md#s2-30aj-bare-bounded-fcomi-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30aj--bounded-fcomi-st0-st1-to-rflags), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ak evidence:** Bare `DF F1` FCOMIP reuses the bounded canonical-Normal compare preflight, maps greater/less/equal to cleared relation bits/CF/ZF, defines all `0x8d5` RFLAGS bits, clears only FSW.C1, empties the actual old physical TOP tag and advances TOP with wrap. It preserves old physical ST(1)'s Valid tag, all register payloads and unrelated state. Coverage includes all TOP×relation×16 initial FSW condition combinations, all 64 initial affected-flag masks with definedness, bad used tags/classes, environment and fetch/truncation/STOP/budget atomicity, exhaustive DF decoding/prefix rejection, exact `dff1`/`dff1dfe0` fixture schema/CLI vectors with actual bad old ST(1) tag, and explicit native denial of `Operation::Fcomip`/DF F1. General x87 exception/special-value behavior and native execution remain unverified. See [S2-30ak contract](specs/CPU_CONTRACT.md#local-s2-30ak-bounded-fcomip-st0-st1-df-f1), [plan](plans/STAGE_2.md#s2-30ak-bare-bounded-fcomip-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30ak--bounded-fcomip-st0-st1-to-rflags-and-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30am evidence:** Bare DF E9 FUCOMIP adds the bounded canonical-Normal compare/pop slice, with TOP×relation×16-condition and 64-RFLAGS-mask matrices, actual physical TOP=5/RHS=6 invalid-tag fixture, exact schema/CLI boundary vectors, exhaustive DF allowlist/prefix checks and explicit native denial. These checks do not establish unordered values, x87 exception delivery, physical x86 execution, or full x87 semantics; see [S2-30am report](reports/STAGE_2_REPORT.md#s2-30am--bounded-fucomip-st0-st1-to-rflags-and-pop) and [CPU contract](specs/CPU_CONTRACT.md#local-s2-30am-bounded-fucomip-st0-st1-df-e9).
- **S2-30al evidence:** Bare `DB E9` FUCOMI preflights canonical-Normal old ST(0)/ST(1) through the bounded integer comparator, maps ordered relations to cleared relation bits/CF/ZF, defines RFLAGS `0x8d5`, clears only FSW.C1 and preserves all other state. Coverage spans TOP×relation×FSW condition combinations, 64 initial flag masks with definedness, invalid operand tags/classes/environment, execution boundaries, exhaustive DB/prefix decoding, exact `dbe9`/`dbe9dfe0` schema/CLI fixtures with the real old ST(1) Empty-tag rejection, `dbf8dfe0` schema rejection (DB F8 remains outside the indexed group) and native denial. QNaN unordered behavior, exception delivery and native x87 execution remain outside this slice. See [S2-30al contract](specs/CPU_CONTRACT.md#local-s2-30al-bounded-fucomi-st0-st1-db-e9), [plan](plans/STAGE_2.md#s2-30al-bare-bounded-fucomi-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30al--bounded-fucomi-st0-st1-to-rflags), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa::x87`, S2-29–S2-39.
- **Статус:** открыта. S2-29a–s наращивают bounded raw/finite helpers: representation/classification, finite normalization, exact alignment/sum/product/quotient, mathematical rounding, normal/subnormal packing и композиции arithmetic→one-round result. S2-29u adds the p64-aware large-gap binary80 addition bridge, S2-29v adds pure finite square root, S2-29w adds exact power-of-two finite scaling, S2-29x adds exact truncated finite remainder, and S2-29y adds exact nearest-even finite remainder; these remain bounded mathematical helpers and leave architectural x87 instruction behavior open. S2-30ba additionally copies canonical Special-tagged binary80 SNaNs and S2-30bb extends that same raw-copy gate to canonical Special-tagged binary80 subnormals; physical/native x87 acceptance remains open. S2-30a–c — ограниченные environment/status instructions; S2-30d–g — exact-only FADD/FADDP; S2-30h–m — exact-only FSUB/FSUBP/FSUBR/FSUBRP; S2-30n–p — exact-only register FMUL/FMULP; S2-30q–v — exact-only FDIV/FDIVR/FDIVP/FDIVRP directions with canonical Normal results; S2-30w adds bare DD D8+i raw-copy FSTP with a canonical-Normal Valid source and explicit physical TOP pop; S2-30x adds bare DD D0+i raw-copy FST without changing TOP; S2-30y adds bare D9 E0 FCHS with a canonical-Normal Valid ST(0) source and sign-bit-only mutation; S2-30z adds bare D9 E1 FABS with the same source gate and sign-bit clear; S2-30aa adds bare D9 E4 FTST with that source gate and sign-derived replacement of all four condition-code bits. S2-30ag adds bare DD E1 FUCOM ST(0),ST(1) for canonical Normals with status-only commit and no pop. FADD/FSUB bounded forms require equal exponents, while FMUL and FDIV permit differing exponents; all require existing x87 state and canonical Normal used operands. S2-30u adds bare `DC F0+i` as `ST(0)/ST(i)` into physical ST(i), without pop. Эти срезы не закрывают общую x87 instruction semantics.
- **Суть и последствия:** `divide_finite_to_binary80`, `multiply_finite_to_binary80`, `round_pack_exact_magnitude`, `round_pack_exact_coefficient` и addition helpers завершают только bounded математическое округление и кодирование конечных binary80 результатов; это не реализует guest FMUL или остальные общие x87 instruction semantics. Division/product reject zero inputs; coefficient/add/magnitude helpers reject exact zero, while add also rejects two zero operands, unequal exponents in the equal-exponent path, and finite-range overflow. Signed zero is preserved for nonzero tiny rounded division/product results. No helper selects an architectural zero sign. Helpers do not implement x87 RC/PC/control-word semantics, architectural underflow/precision effects, special values, general guest FDIV/FADD/FSUB/FMUL semantics (except existing exact-only bounded instruction slices), flags/exceptions. FNINIT, FNSTSW AX and FNCLEX remain bounded local forms: CR0.EM/TS and #NM are not modeled; FNINIT does not model preceding WAIT FINIT or pending-exception delivery. FNSTSW AX copies raw FSW, while C0/C1/C2/C3 are architecturally undefined; deterministic snapshot retention is only local model policy. FNCLEX clears FSW[0:7] and FSW[15]; pending-exception check and delivery are not modeled. MMX/SSE shared state, native-state observation, and complete save/restore remain open. Helpers do not imply general x87 execution or FPU feature availability; x87 snapshot remains a bounded local profile.
- **Основание:** локальные literal vectors покрывают exact finite intermediates, mathematical rounding, normal/subnormal bytes и композиции finite division/product/coefficient/addition с one-round pack. Для S2-29s проверяются even-magnitude equivalence, odd product tails below/at/above ties во всех режимах/знаках, normal carry, minimum-normal/subnormal promotion, minimum/half-minimum subnormal, signed underflow zero, exponent/finite rejection, zero product и input immutability. Это не проверяет FMUL instruction semantics. Для S2-29q проверяются ±1+±1, signed normal ties в четырёх режимах, минимум/максимум subnormal границы, zero-neutral для normal/subnormal/pseudo-denormal в обоих порядках, max-finite plus zero, overflow rejection, cancellation, все zero-sign pairs, unequal exponents и неизменность input/provenance. Сумма двух significands ограничена `2^65-2`, поэтому synthetic carry-to-four недостижим; exact subnormal sums кратны minimum subnormal, поэтому inexact subnormal tails и округление в underflow zero недостижимы. S2-30d literal vectors покрывают исходный exact-only FADD commit/reject профиль. S2-30e добавляет независимые byte vectors для всех i=0..7 и разных TOP, alias case i=0 и wrap cases i=1/TOP=7, i=7/TOP=7, empty unrelated tags при i=0, per-index full-state reject, exhaustive decode/prefix/truncation, schema acceptance и native fail-closed coverage. Full x87 architectural behavior не проверялся. См. [отчёт S2-29s](reports/S2_29S_X87_PRODUCT_PACK.md), [отчёт этапа 2 S2-29q](reports/STAGE_2_REPORT.md#s2-29q--compose-same-exponent-finite-addition-with-one-round-binary80-packing-partial), [CPU contract S2-29q](specs/CPU_CONTRACT.md#local-s2-29q-same-exponent-finite-addition-to-one-rounded-binary80-result), [план S2-29q](plans/STAGE_2.md#s2-29q-compose-same-exponent-finite-addition-with-one-round-binary80-packing-partial), предыдущие [S2-29p](reports/STAGE_2_REPORT.md#s2-29p--round-exact-coefficient-to-one-binary80-result-partial) и [S2-29o](reports/STAGE_2_REPORT.md#s2-29o--compose-finite-division-with-single-round-binary80-packing-partial), и [исходник x87.rs](../crates/od-isa/src/x87.rs).
- **S2-31a evidence:** bare `D9 FC` preflights only a Valid old physical TOP with canonical-Normal `e=0..62`, one of four PC64/all-masked RC control words, and `FSW & 0x80df == 0`. Integer-only quotient/remainder rounding and binary80 packing cover all TOP×RC×sign×exact/tie/adjacent-boundary vectors and both signed `2^63` outputs; PE sticky, C1 increment, local C0/C2/C3 retention, complete unrelated-state preservation, invalid source/environment and execution-boundary atomicity are checked. The schema admits `D9 FC` and exact `D9 FC DF E0` (FNSTSW AX); literal CLI fixtures cover tie-to-even, status composition and an invalid physical TOP tag. Native admission explicitly rejects FRNDINT. This does not complete S2-31 or establish general x87 rounding/exceptions. See [S2-31a contract](specs/CPU_CONTRACT.md#local-s2-31a-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31a-bare-bounded-frndint-d9-fc-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31a--bounded-frndint-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31dc evidence:** signed-zero support requires the Zero tag and exact raw ±0; success preserves ten bytes, Zero/TOP, PE, C0/C2/C3 and all unrelated state, clears C1, records FIP, advances RIP and retires once. The 256-case sign×FCW×TOP×PE×C1 matrix, malformed tag/payload rejects, unchanged Normal regression, and literal negative-zero FNSTSW fixture are covered. Existing FNCLEX/decode and native/JIT denial remain in force. Physical acceptance is deferred to the combined final S2-31 substage. See [S2-31dc contract](specs/CPU_CONTRACT.md#local-s2-31dc-canonical-signed-zero-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dc-admit-signed-zero-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dc--canonical-signed-zero-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dd evidence:** bounded `D9 FC` accepts Valid canonical-Normal inputs with unbiased exponent 63..16383 and returns the original bytes before shift/round/pack; it clears only C1 among status bits, preserves sticky PE/C0/C2/C3 and all raw/tag/stack state, updates FIP/RIP, and retires once. Matrices cover the requested exponent/significand/sign/TOP/FCW/PE/C1 combinations; boundary tests distinguish fractional e=62 rounding from exact e=63 identity. A literal FNSTSW CLI fixture covers composition. Native/JIT remain closed. Physical x87 acceptance and general exception behavior remain deferred to combined S2-31 acceptance. See [S2-31dd contract](specs/CPU_CONTRACT.md#local-s2-31dd-exact-integer-normal-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dd-admit-exact-integer-normal-values-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dd--exact-integer-normal-values-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31de evidence:** bare `D9 FC` now admits only Special-tagged canonical signed binary80 infinity (J=1, exponent `0x7fff`, zero fraction), preserving all ten bytes/TOP/tag and PE/C0/C2/C3 while clearing C1, updating FIP/RIP and retiring once. The 256-case sign/FCW/TOP/PE/C1 matrix, malformed Special/tag/environment and boundary rejects, literal `FLD m80fp`→`FRNDINT`→`FNSTSW AX` CLI fixture, JIT denial and existing native x87 deny are covered. Physical x86 acceptance and general x87 exception behavior remain open under ISSUE-019. See [S2-31de contract](specs/CPU_CONTRACT.md#local-s2-31de-canonical-signed-infinity-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31de-admit-canonical-signed-infinity-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31de--canonical-signed-infinity-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31df evidence:** bare `D9 FC` admits only Special-tagged canonical binary80 QNaNs with exponent `0x7fff`, J=1 and bit 62=1; sign and lower 62 payload bits are preserved exactly. The 1,024-case significand/sign/FCW/TOP/PE/C1 matrix, SNaN/pseudo-NaN/tag/control/status rejects, execution-boundary atomicity, literal FLD→FRNDINT→FNSTSW fixture, JIT denial and existing native x87 deny are covered. Physical x86 acceptance and general x87 exception behavior remain open under ISSUE-019. See [S2-31df contract](specs/CPU_CONTRACT.md#local-s2-31df-canonical-quiet-nan-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31df-admit-canonical-quiet-nan-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31df--canonical-quiet-nans-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dg evidence:** bare `D9 FC` admits only Special-tagged canonical binary80 SNaNs (E=`0x7fff`, J=1, Q=0, lower payload nonzero), sets only Q, adds sticky IE without PE, clears C1 and preserves PE/C0/C2/C3 and unrelated state. The 1,024-case matrix, zero-payload/QNaN/J=0/tag/control/status/prefix/fetch/truncation/STOP/budget controls, IE re-entry/FNSTSW/FNCLEX checks, literal FLD→FRNDINT→FNSTSW→FNCLEX→FRNDINT fixture, and JIT/native denial are recorded. Physical x86 behavior and general exception delivery remain open under ISSUE-019. See [S2-31dg contract](specs/CPU_CONTRACT.md#local-s2-31dg-canonical-signaling-nan-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dg-quiet-canonical-signaling-nan-in-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dg--canonical-signaling-nans-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dh evidence:** bare `D9 FC` admits historical IE using `FSW & 0x80de == 0`, while DE/ZE/OE/UE/SF/ES/B remain rejected. Coverage crosses the admitted zero, Normal, infinity, QNaN and SNaN classes with signs, all FCWs/TOPs, IE/PE and old C1; current inexact rounding still sets PE, current SNaN still sets IE and quiets, and IE+forbidden-status cases reject atomically. A quieted SNaN re-enters successfully without FNCLEX. The existing FNSTSW/FNCLEX fixture and JIT/native denial remain in place. Physical x86/native runtime behavior remains deferred under ISSUE-019. See [S2-31dh contract](specs/CPU_CONTRACT.md#local-s2-31dh-historical-sticky-ie-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dh-admit-historical-sticky-ie-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dh--historical-sticky-ie-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dj evidence:** `D9 FC` now admits historical sticky DE under the narrow status mask `0x80dc`; DE remains set after FRNDINT, current rounding and IE/PE behavior is unchanged, and the four accepted all-masked FCWs are unchanged. Interpreter vectors cover representative Normal exponents and all supported classes with sign/RC/TOP/sticky IE/DE/PE/prior-C1 combinations, and forbidden status bits paired with admitted flags remain atomic rejects. Literal m32/m64 minimum-subnormal FLD→FRNDINT→FNSTSW fixtures verify the loader's canonical-Normal+DE state flows through rounding. JIT/native denial remains in place. Physical x86 acceptance and general x87 exception delivery remain open; complete them in the combined final S2-31 acceptance substage. See [S2-31dj contract](specs/CPU_CONTRACT.md#local-s2-31dj-historical-sticky-de-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dj-admit-historical-sticky-de-to-bounded-frndint-d9-fc), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dk evidence:** bare `D9 FC` changes only its local FSW gate from `0x80dc` to `0x80cc`, admitting and preserving historical UE. Independent interpreter vectors cover UE with every IE/DE/PE combination, prior C1, all four RCs, every TOP, and exact Normal, inexact Normal and canonical SNaN inputs. Exact UE-only does not create PE; current inexact rounding adds PE; SNaN adds IE without PE. ZE/OE/SF/ES/B remain atomic rejects with UE and admitted flags. Literal CLI fixtures cover seeded UE and FST m32 underflow→FRNDINT→FNSTSW composition. Native/JIT remain closed. Physical x86/Mac mini acceptance and general x87 exception delivery remain open under ISSUE-019. See [S2-31dk contract](specs/CPU_CONTRACT.md#local-s2-31dk-historical-sticky-ue-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dk-admit-historical-sticky-ue-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dk--historical-sticky-ue-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dl evidence:** bare `D9 FC` changes only its local FSW gate from `0x80cc` to `0x80c4`, admitting and preserving historical OE. Independent interpreter vectors cross OE with IE/DE/UE/PE, prior C1, all four RCs, every TOP, and exact/fractional Normal plus canonical SNaN. Exact/identity sources do not create PE; current inexact rounding adds PE and SNaN adds IE without PE. ZE/SF/ES/B remain atomic rejects when OE and admitted stickies are set. Literal CLI fixtures cover seeded OE and FST m32 overflow→FRNDINT→FNSTSW, including final OE+PE with C1 cleared; direct producer status is separately covered by the existing FST m32 overflow unit matrix. Native/JIT remain closed. Physical x86/Mac mini acceptance and general x87 exception delivery remain open under ISSUE-019. See [S2-31dl contract](specs/CPU_CONTRACT.md#local-s2-31dl-historical-sticky-oe-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dl-admit-historical-sticky-oe-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dl--historical-sticky-oe-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dm evidence:** the bounded FRNDINT gate changes only from `0x80c4` to `0x80c0`, admitting historical ZE while retaining independent IE/DE/OE/UE/PE and rejecting SF/ES/B. The existing class/sign/RC/TOP/prior-C1 matrix crosses ZE with all 32 combinations of the other sticky flags; exact inputs retain ZE without creating PE, fractions add PE, and current SNaN adds IE without PE. A literal seeded `D9 FC DF E0` fixture begins at FSW=0x000c (ZE+OE), explicitly documents that the input state was seeded with no guest producer, and asserts AX/FSW=0x000c. No FDIV-zero producer/domain or JIT/native admission changed. Physical x86/Mac mini acceptance and general x87 exception delivery remain open under ISSUE-019. See [S2-31dm contract](specs/CPU_CONTRACT.md#local-s2-31dm-historical-sticky-ze-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dm-admit-historical-sticky-ze-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dm--historical-sticky-ze-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dn evidence:** bounded `D9 FC` uses `FSW & 0x8080 == 0` plus an explicit SF-without-IE reject. The independent 32,768-case matrix crosses historical SF+IE with DE/ZE/OE/UE/PE, all eight source classes, signs, four FCWs, TOP and prior C1; SF+IE and other historical sticky bits survive, while C1 is recomputed from the current result. SF-only rejects include current SNaN; ES/B and existing Empty/mismatch/raw-malformed controls remain rejected. The literal `D9 FC DF E0` project_fixture starts at source 1.0, FCW 0x037f and FSW 0x0241, observing AX=0x0041. IE+SF are seeded in the initial snapshot, are not guest-produced, and the fixture is not native evidence. No ES/B or pending/#MF/WAIT semantics changed; JIT/native remain closed. Physical x86 and general exception-delivery evidence remain open under ISSUE-019. See [S2-31dn contract](specs/CPU_CONTRACT.md#local-s2-31dn-historical-sfie-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-31dn--historical-sticky-sf-with-ie-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31di evidence:** bare `D9 FC` admits Valid canonical-Normal inputs with unbiased exponent `-16382..=-1` through comparison-only rounding. Nearest-even returns one only strictly above 0.5 at e=-1; directed results use sign/direction; all sources are inexact, set sticky PE and set C1 exactly for magnitude one. Rounded signed zeros carry Zero tags. The sign/FCW/TOP/exponent/significand/IE-PE/C1 matrix, repeated FRNDINT, atomic wrong-tag/status controls, literal minimum-normal→negative-zero→FNSTSW fixture, and continued JIT/native denial are recorded. Physical x86/Mac mini acceptance remains deferred to the combined final S2-31 substage; broader x87 exception delivery remains unverified. See [S2-31di contract](specs/CPU_CONTRACT.md#local-s2-31di-small-canonical-normal-frndint-d9-fc), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-31di--small-canonical-normal-values-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31b evidence:** bare `DB 18` implements a bounded `FISTP m32int [RAX]` for canonical-Normal `e=0..31`, all-masked PC64 FCW values and `FSW & 0x80df == 0`. Integer-only quotient rounding rejects signed-i32 overflow before access; the full canonical48 Dword range is checked before one atomic `write_integer` call. Tests cover exact unaligned four-byte mutation/canaries, no data read, exactly one write, all four RC modes on exact/inexact signed values, literal raw binary80 vectors on both rounding-induced i32 boundaries, overflow-before-access, write permission failure and injected #PF with unchanged CPU state. Literal fixtures admit `DB 18`, its exact `DB 18 DF E0` status composition, and unsupported tag/overflow cases. Native execution explicitly denies the operation. Flat fixture regions do not model MMU translation, page walks, MMIO, SMP, or architectural exception delivery. See [S2-31b contract](specs/CPU_CONTRACT.md#local-s2-31b-bounded-fistp-m32int-rax-db-18), [plan](plans/STAGE_2.md#s2-31b-bare-bounded-fistp-m32int-rax-db-18-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31b--bounded-fistp-m32int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31c evidence:** bare `DB 10` implements bounded non-pop `FIST m32int [RAX]` with the S2-31b integer conversion domain, four RC modes, post-rounded signed-i32 bounds, full canonical48 validation and one atomic Dword store. Successful commit changes only PE/C1, FIP/FDP, RIP and retirement while preserving TOP, every physical tag, all payloads and unrelated state. Literal fixtures admit `DB 10` and exact `DB 10 DF E0`; interpreter checks cover signed half-way rounding, overflow-before-access, write fault atomicity and no-read/single-write behavior. Native execution explicitly denies the operation. Flat fixture RAM does not establish MMU translation, page walks, MMIO, SMP, or architectural exception delivery. See [S2-31c contract](specs/CPU_CONTRACT.md#local-s2-31c-bounded-fist-m32int-rax-db-10), [plan](plans/STAGE_2.md#s2-31c-bare-bounded-fist-m32int-rax-db-10-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31c--bounded-fist-m32int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31d evidence:** bare `DF 18` implements bounded `FISTP m16int [RAX]` for canonical-Normal old physical TOP with exponent 0..15, exact PC64/all-masked FCW values, and `FSW & 0x80df == 0`. The shared integer quotient/remainder converter checks signed-i16 bounds after rounding; success validates canonical48 across two bytes, performs one atomic Word write without a read, then pops TOP and commits PE/C1/FIP/FDP/RIP once. Literal fixtures cover all RC modes with both signs, ties, exact/rounded limits, `DF 18 DF E0`, overflow and invalid tag/status/configuration. Interpreter checks include full CPU/payload preservation, no-access rejects, permission/#PF atomicity and canonical-range rejection. Native admission denies this operation. Flat fixtures do not establish MMU, MMIO, SMP, physical execution, or general x87 exception behavior. S2-31ck supersedes the original finite-overflow rejection with integer-indefinite/IE commit for admitted DF 18/DF 10 forms. See [S2-31d contract](specs/CPU_CONTRACT.md#local-s2-31d-bounded-fistp-m16int-rax-df-18), [plan](plans/STAGE_2.md#s2-31d-bare-bounded-fistp-m16int-rax-df-18-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31d--bounded-fistp-m16int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31e evidence:** bare `DF 10` implements bounded non-pop `FIST m16int [RAX]` using S2-31d's conversion domain and signed-i16 range after RC rounding. Success validates canonical48 across both bytes, makes one atomic Word write without reading data, then commits PE/C1/FIP/FDP/RIP and retires once while preserving TOP, every tag and all payloads. Literal fixtures cover `DF 10`, exact `DF 10 DF E0`, no-pop state, tie/boundaries, overflow and invalid tag/status/control/exponent. Interpreter checks include all RC modes/signs, permission/#PF atomicity and canonical-range rejection. Native admission explicitly denies this operation. Flat fixtures do not establish MMU, MMIO, SMP, physical execution or general x87 exception behavior. S2-31ck supersedes the original finite-overflow rejection with integer-indefinite/IE commit for admitted DF 10/DF 18 forms. See [S2-31e contract](specs/CPU_CONTRACT.md#local-s2-31e-bounded-fist-m16int-rax-df-10), [plan](plans/STAGE_2.md#s2-31e-bare-bounded-fist-m16int-rax-df-10-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31e--bounded-fist-m16int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31f evidence:** bare `DF 38` implements a bounded `FISTP m64int [RAX]` with canonical-Normal old physical TOP, exponent 0..63, exact PC64/all-masked FCW values and `FSW & 0x80df == 0`. Integer quotient/remainder rounding checks signed-i64 limits in `u128` before narrowing and encodes through unsigned Qword two's complement; exponent 63 has zero shift and zero remainder. Success validates all eight canonical48 bytes, makes one atomic Qword write without reading data, then pops old TOP and commits PE/C1/FIP/FDP/RIP once. Literal fixtures cover exact signed boundaries, DF 38 DF E0, Qword canaries and +2^63 rejection; interpreter vectors cover all RC/sign ties, all RCs for -2^63 and `2^63 - 0.5`, -(2^63+1), exponent 64, canonical-range rejection and write/#PF atomicity. Native admission denies the operation. Overflow returns local Unsupported instead of an indefinite integer; architectural exception delivery and physical x86 behavior remain unmodeled. See [S2-31f contract](specs/CPU_CONTRACT.md#local-s2-31f-bounded-fistp-m64int-rax-df-38), [plan](plans/STAGE_2.md#s2-31f-bare-bounded-fistp-m64int-rax-df-38-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31f--bounded-fistp-m64int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31g evidence:** bare `D9 38` stores any schema-valid raw FCW through one canonical48-checked Word write, accepts pending FSW/status bits and Empty tags, and preserves the complete x87 snapshot plus GPR/RFLAGS. Exact-only schema/composition and native-deny coverage are added; workspace tests cover canaries, second-byte store-fault atomicity and execution boundaries. `cargo test --workspace --offline`, workspace check, fmt and diff checks passed. Layout check still reports pre-existing `.serena` as unmapped; it was left untouched. General x87 exception delivery and physical behavior remain unverified, so this umbrella issue stays open.
- **S2-31h evidence:** bare `DD 38` stores raw pre-operation FSW with one canonical48-checked Word write and no data read. It accepts arbitrary schema-valid FCW/FSW and Empty tags, preserves every x87/GPR/RFLAGS field, and explicitly denies native admission. Literal `0000`/mixed/`ffff`, composition, sentinel/canary and canonical-crossing fixtures plus interpreter checks for permission/backend/fetch/stop/budget failures are documented in the [S2-31h CPU contract](specs/CPU_CONTRACT.md#local-s2-31h-bounded-fnstsw-m2byte-rax-dd-38), [plan](plans/STAGE_2.md#s2-31h-bare-bounded-fnstsw-m2byte-rax-dd-38-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31h--bounded-fnstsw-m2byte-rax-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md). This does not verify #MF/#NM/CR0 behavior, MMU/MMIO, or physical execution; the umbrella issue remains open.
- **S2-31i evidence:** bare `D9 28` accepts only the four bounded FCWs after validating the old environment and one canonical48 Word read. All 16 FCW pairs, RC-driven FRNDINT composition, invalid old/new state, memory and execution boundaries are covered; native admission remains closed. Pending/unmasked exception delivery, WAIT, `#MF`, `#NM`/CR0, MMU/MMIO, and physical behavior remain unverified. See the [S2-31i CPU contract](specs/CPU_CONTRACT.md#local-s2-31i-bounded-fldcw-m2byte-rax-d9-28), [plan](plans/STAGE_2.md#s2-31i-bare-bounded-fldcw-m2byte-rax-d9-28-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31i--bounded-fldcw-m2byte-rax-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31j evidence:** the bounded `D9 18` binary80-to-binary32 slice uses integer quotient/remainder rounding, admits only canonical Normal exponents -126..126 and finite normal results, and performs one canonical48-checked atomic Dword write before the physical TOP pop. Literal tie/RC/sign/exponent vectors, all TOPs, C1/PE behavior, canaries, canonical/store failure atomicity, and exact `D9 18`/`D9 18 DF E0` CLI fixtures cover the local contract. Hardware exception priority/delivery, native x87 state observation, MMU/MMIO, SMP and physical x86 behavior remain unverified; S2-31 remains open. See the [S2-31j CPU contract](specs/CPU_CONTRACT.md#local-s2-31j-bounded-fstp-m32fp-rax-d9-18), [plan](plans/STAGE_2.md#s2-31j-bare-bounded-fstp-m32fp-rax-d9-18-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31j--bounded-fstp-m32fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31m evidence:** exact bare `DD 10` implements bounded no-pop `FST m64fp [RAX]` by reusing the unchanged S2-31l binary80-to-binary64 helper. Literal RC/tie/exponent vectors and all-TOP payload/tag preservation checks cover the unrounded ST(0); Qword/no-read, canonical-range, backend/#PF atomicity, invalid preflight and execution-boundary checks are documented with exact `DD 10` and `DD 10 DF E0` fixtures. Native admission explicitly denies the operation. The -1022..1022 exponent interval and condition-code preservation/C1 behavior are local deterministic policies. Physical x86 state/exception behavior, MMU/MMIO/SMP and the broader S2-31 exception model remain unverified; ISSUE-019 stays open. See [S2-31m CPU contract](specs/CPU_CONTRACT.md#local-s2-31m-bounded-fst-m64fp-rax-dd-10), [plan](plans/STAGE_2.md#s2-31m-bare-bounded-fst-m64fp-rax-dd-10-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31m--bounded-fst-m64fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31n evidence:** exact bare `DB 00` pushes a signed Dword integer into physical `(TOP+7)&7` after the exact FCW/FSW/Empty-tag gates and one canonical48-checked Dword read. Integer-only packing is covered by independent binary80 literals for zero, ±1/2/3, `2^24±1`, and i32 extrema; matrices cover all TOP and RC values, stack wrap and unrelated tag/payload preservation. Read faults/backends, invalid preflight, canonical range, fetch/truncation/STOP/budget atomicity, exhaustive DB/prefix decoding, exact `DB 00`/`DB 00 DF E0` fixture schema and native denial are covered. C0/C2/C3 retention is a local policy. General exception delivery, hardware fault priority, MMU/MMIO/SMP and physical x86 behavior remain unverified; S2-31 stays partial and ISSUE-019 remains open. See [S2-31n CPU contract](specs/CPU_CONTRACT.md#local-s2-31n-bounded-fild-m32int-rax-db-00), [plan](plans/STAGE_2.md#s2-31n-bare-bounded-fild-m32int-rax-db-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31n--bounded-fild-m32int-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31o evidence:** exact bare `DF 00` pushes a signed Word integer through the S2-31n canonical48/Empty-destination preflight and one typed Word read. Sign extension feeds the shared integer-only i64→binary80 packer; zero receives the Zero tag. Literal binary80 values cover zero, ±1/2/3 and signed-word extrema over all TOP×RC settings, including `0x8000`/`0xffff`, stack wrap, stale Empty payload, unrelated tags/payloads, sticky PE and C1 clearing. Unaligned read/canaries prove a two-byte access; canonical range, read #PF/backend error, invalid preflight and fetch/truncation/STOP/budget failures preserve state. Exhaustive DF decode adds only DF 00 while retaining DF 10/18/38/E0/E9/F1; schema/CLI fixtures cover exact DF 00 and DF 00 DF E0; native admission denies the form. C0/C2/C3 retention is local policy. General exception delivery, hardware fault priority, MMU/MMIO/SMP and physical x86 behavior remain unverified; S2-31 stays partial and ISSUE-019 remains open. See [S2-31o CPU contract](specs/CPU_CONTRACT.md#local-s2-31o-bounded-fild-m16int-rax-df-00), [plan](plans/STAGE_2.md#s2-31o-bare-bounded-fild-m16int-rax-df-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31o--bounded-fild-m16int-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31p evidence:** exact bare `DF 28` pushes a signed Qword through the shared FILD gates and one complete canonical48-checked Qword read. The generalized integer-only i64→binary80 packer uses `unsigned_abs`, handles `i64::MIN`, emits the exact zero form, and does not depend on RC. Independent literals cover zero, ±1/2/3, i64 extrema, `2^53±1`, `2^53`, and signed values around `2^62` over all TOP×RC pairs. Checks cover occupied/Empty old ST(0), stale Empty payload, other tags/payloads, sticky PE/C1, unaligned canaries/exact read width, noncanonical and crossing ranges, range overflow, read #PF/backend errors, invalid gates, and fetch/truncation/STOP/budget atomicity. Exhaustive DF decode preserves existing DF and DB forms; exact `DF 28`/`DF 28 DF E0` fixture/schema/CLI checks validate bytes independently of case name, and native admission explicitly denies FILD m64. The m16/m32 regression tests exercise the shared packer. General x87 exception delivery, MMU/MMIO/SMP and physical x86 behavior remain unverified; S2-31 stays partial. See [S2-31p CPU contract](specs/CPU_CONTRACT.md#local-s2-31p-bounded-fild-m64int-rax-df-28), [plan](plans/STAGE_2.md#s2-31p-bare-bounded-fild-m64int-rax-df-28-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31p--bounded-fild-m64int-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31r evidence:** exact bare `DB 28` loads canonical-Normal m80fp from `[RAX]` after the shared bounded push gates and complete canonical48 ten-byte preflight. One exact slice read preserves raw bytes; all non-Normal classes reject after the read without CPU commit. Literal interpreter vectors cover every TOP and FCW, raw positive/negative fractional and exponent-boundary values, PE/C1 and local C0/C2/C3 policy, other tags/payloads, no-write behavior, invalid gates/ranges/#PF/backend errors and execution boundaries. Exhaustive DB decode preserves existing forms and rejects DB 30; exact DB 28, DB 28 DF E0 and invalid-class no-commit schema/CLI fixtures are added. Native x87 admission, physical behavior, general exception delivery, MMU/MMIO/SMP remain unverified; S2-31 remains partial. See [S2-31r CPU contract](specs/CPU_CONTRACT.md#local-s2-31r-bounded-fld-m80fp-rax-db-28), [plan](plans/STAGE_2.md#s2-31r-bare-bounded-fld-m80fp-rax-db-28-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31r--bounded-fld-m80fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31s evidence:** exact bare `D9 00` loads binary32 Normal `[RAX]` into the pushed physical x87 slot by integer field expansion. Independent raw binary80 literals cover ±1, adjacent fractions, minimum/maximum Normal and exponent 127 across every TOP×RC combination; tests also cover sticky PE/C1 policy, exact unaligned Dword access/canaries, invalid classes after one read, preflight gates, canonical crossing/overflow, #PF/backend errors and execution boundaries. Exact `D9 00`, `D9 00 DF E0`, and invalid-class CLI/schema fixtures are included; native admission denies the operation. Special values, general exception delivery, MMU/MMIO/SMP and physical behavior remain unverified. S2-31 and ISSUE-019 stay open. See [S2-31s CPU contract](specs/CPU_CONTRACT.md#local-s2-31s-bounded-fld-m32fp-rax-d9-00), [plan](plans/STAGE_2.md#s2-31s-bare-bounded-fld-m32fp-rax-d9-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31s--bounded-fld-m32fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31t evidence:** exact bare `DD 00` expands binary64 Normal `[RAX]` into the pushed x87 slot using integer field conversion, including unbiased exponent +1023. Independent raw binary80 tests cover ±1, adjacent fractions and Normal endpoints over every TOP×FCW combination; exact unaligned Qword access, class rejection, push gates, state retention, exhaustive DD decoding and literal CLI/schema fixtures are included. Native admission denies the operation. Access fault and execution-boundary cases are tracked in the S2-31t interpreter coverage; special values, general exception delivery, MMU/MMIO/SMP and physical behavior remain outside the current contract. S2-31 and ISSUE-019 stay open. See [S2-31t CPU contract](specs/CPU_CONTRACT.md#local-s2-31t-bounded-fld-m64fp-rax-dd-00), [plan](plans/STAGE_2.md#s2-31t-bare-bounded-fld-m64fp-rax-dd-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31t--bounded-fld-m64fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31u historical evidence:** extends exact bare `DD 00` to binary64 +0/-0 after the existing single Qword read; it emits signed binary80 zero and the Zero tag. Normal behavior is unchanged. At the S2-31u stage, positive/negative nonzero minimum subnormals and Inf/NaN remained Unsupported without CPU commit; S2-31z now supports masked nonzero subnormals. See current [S2-31z evidence](#s2-31z-evidence). Independent signed-zero vectors cover TOP×RC; rejection vectors verify signed minimum subnormals after one read. Sticky PE/C1, wrap and untouched state are covered. Success and rejection schema/CLI fixtures use positive zero and positive minimum subnormal respectively. Native admission remains closed. The S2-31t report remains historical and records its original Normal-only scope. General x87 exception delivery, MMU/MMIO/SMP and physical acceptance remain open under ISSUE-019; the final combined S2-31 substage still owns physical acceptance. See [S2-31u CPU contract](specs/CPU_CONTRACT.md#local-s2-31u-bounded-fld-m64fp-rax-signed-zero-extension), [plan](plans/STAGE_2.md#s2-31u-extend-bounded-fld-m64fp-rax-dd-00-to-signed-zero), [report](reports/STAGE_2_REPORT.md#s2-31u--bounded-fld-m64fp-signed-zero-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31v historical evidence:** at that point, exact bare `D9 00` accepted binary32 +0/-0, and rejected nonzero subnormals, infinity and NaNs after one read. The S2-31y current contract extends masked binary32 subnormals and replaces the invalid-class subnormal fixture with infinity. Preserve this row and report as the historical S2-31v result; see the current [S2-31y evidence](#s2-31y-evidence).
- **S2-31w evidence:** exact bare `DB 28` now accepts raw binary80 +0/-0 after the unchanged one-read preflight, preserves their ten bytes and assigns `X87Tag::Zero`; Normal remains byte-preserving with Valid. Both signs of minimum nonzero subnormal and other special classes remain Unsupported after one read with complete CPU-state preservation. All TOP×FCW literal matrices, exact +0/−0 CLI fixtures, and the nonzero-subnormal schema exception predicate are covered. Intel SDM093 FLD m80fp specifies no `#D` for this operand; broader denormal/unmasked handling is deferred. Native admission and physical acceptance remain closed; S2-31 and ISSUE-019 stay open. See [S2-31w CPU contract](specs/CPU_CONTRACT.md#local-s2-31w-bounded-fld-m80fp-rax-db-28-signed-zero-extension), [plan](plans/STAGE_2.md#s2-31w-extend-bounded-fld-m80fp-rax-db-28-to-signed-zero), [report](reports/STAGE_2_REPORT.md#s2-31w--bounded-fld-m80fp-signed-zero-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31x historical evidence:** exact bare `DB 28` accepted canonical binary80 subnormals, preserving all ten bytes and assigning `X87Tag::Special` after the same one-read preflight. Literal signed minimum/largest/interior vectors covered all TOP×FCW combinations with unchanged PE/status policy; positive-minimum and negative-largest CLI fixtures covered bare and FNSTSW composition paths. At that stage pseudo-denormal remained Unsupported and the narrow schema exception used positive pseudo-denormal; S2-31aa replaced that exception fixture with canonical QNaN, and S2-31ab replaces the current exception fixture with canonical SNaN while adding canonical QNaN loads. Native admission and physical acceptance remain closed, and broader x87 exception delivery remains open under ISSUE-019. See [S2-31x CPU contract](specs/CPU_CONTRACT.md#local-s2-31x-bounded-fld-m80fp-rax-db-28-canonical-subnormal-extension), [plan](plans/STAGE_2.md#s2-31x-extend-bounded-fld-m80fp-rax-db-28-to-canonical-subnormals), [report](reports/STAGE_2_REPORT.md#s2-31x--bounded-fld-m80fp-canonical-subnormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31y-evidence"></a>
- **S2-31y evidence:** at that stage, exact bare `D9 00` accepted nonzero binary32 subnormals under four all-masked FCWs, normalized fractions to canonical Normal binary80, assigned Valid, and set sticky DE. Its FSW gate permitted pre-existing DE (`0x80dd`); FILD and m64/m80 load gates stayed unchanged. Signed minimum/interior/largest vectors covered TOP×FCW, repeated loads, sticky PE/C1, UE/PE non-addition and DE retention. Positive D900 and negative D900/DFE0 fixtures exercised subnormal conversion. The S2-31y invalid fixture/schema used binary32 infinity or NaN; S2-31af temporarily narrowed that predicate to canonical QNaN after adding infinities, and S2-31ah now accepts QNaNs and requires canonical SNaN. Unmasked exception delivery, native admission and physical acceptance remain outside the slice; m80fp subnormal behavior stays distinct with no DE. See [S2-31y CPU contract](specs/CPU_CONTRACT.md#local-s2-31y-bounded-fld-m32fp-rax-d9-00-masked-subnormal-extension), [plan](plans/STAGE_2.md#s2-31y-extend-bounded-fld-m32fp-rax-d9-00-to-masked-subnormals), [report](reports/STAGE_2_REPORT.md#s2-31y--bounded-fld-m32fp-masked-subnormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31z-evidence"></a>
- **S2-31z evidence:** exact bare `DD 00` accepts nonzero binary64 subnormals under the four all-masked FCWs, normalizes their exact fractions to canonical Normal binary80, assigns Valid, and sets sticky DE. Its DD00-only preflight permits pre-existing DE (`0x80dd`); FILD and m80 load gates remain at `0x80df`. Literal signed minimum/interior/largest vectors cover all TOP×FCW pairs, sticky PE/C1, repeated loads and exact Qword reads; CLI fixtures cover positive Dd00 and negative Dd00/DFE0 with FNSTSW. Inf/NaN remain post-read Unsupported, and the schema predicate is exact DD00, budget one and no stop RIP. Unmasked exception delivery and physical acceptance remain outside the slice; native admission stays closed. See [S2-31z CPU contract](specs/CPU_CONTRACT.md#local-s2-31z-bounded-fld-m64fp-rax-dd-00-masked-subnormal-extension), [plan](plans/STAGE_2.md#s2-31z-extend-bounded-fld-m64fp-rax-dd-00-to-masked-subnormals), [report](reports/STAGE_2_REPORT.md#s2-31z--bounded-fld-m64fp-masked-subnormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31aa-evidence"></a>
- **S2-31aa historical evidence:** exact bare `DB 28` accepts canonical signed binary80 infinities only, preserves all ten raw bytes, and assigns Special under the unchanged gates. Its TOP×FCW matrices verify payload/tag, C1 clear, preserved flags and no new status. At that stage all NaNs were rejected and the narrow schema exception used positive QNaN; S2-31ab supersedes the QNaN behavior and switches the exception fixture to SNaN. Positive bare and negative DB28/DFE0 FNSTSW fixtures run in the CLI; JIT/native admission explicitly denies the operation. Physical acceptance remains deferred to the combined final S2-31 substage. See [S2-31aa CPU contract](specs/CPU_CONTRACT.md#local-s2-31aa-bounded-fld-m80fp-rax-db-28-canonical-infinities), [plan](plans/STAGE_2.md#s2-31aa-extend-bounded-fld-m80fp-rax-db-28-to-canonical-infinities), [report](reports/STAGE_2_REPORT.md#s2-31aa--bounded-fld-m80fp-canonical-infinity-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31ab-evidence"></a>
- **S2-31ab evidence:** exact bare `DB 28` also accepts only canonical QuietNaNs (biased exponent `0x7fff`, explicit integer bit and quiet bit set, nonzero fraction), preserves raw bytes, pushes Special, adds no status flags including IE, and preserves PE under the unchanged all-masked FCW/`FSW & 0x80df`/Empty/ten-byte/one-read profile. Independent positive-payload and negative-indefinite literals cover all TOP×FCW pairs. CLI fixtures exercise positive bare QNaN and negative indefinite QNaN with FNSTSW; JIT and native profiles still deny the operation. The narrow exact-DB28/budget-one/no-stop Unsupported exception now uses positive canonical SNaN, and canonical QNaNs, pseudo-NaNs, pseudo-denormals, pseudo-infinities and unnormals do not match it. Mac mini/physical acceptance remains deferred. See [S2-31ab CPU contract](specs/CPU_CONTRACT.md#local-s2-31ab-bounded-fld-m80fp-rax-db-28-canonical-quietnans), [plan](plans/STAGE_2.md#s2-31ab-extend-bounded-fld-m80fp-rax-db-28-to-canonical-qnans), [report](reports/STAGE_2_REPORT.md#s2-31ab--bounded-fld-m80fp-canonical-quietnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ac evidence:** exact bare `DB 28` accepts canonical SignalingNaNs (biased exponent `0x7fff`, explicit integer bit set, quiet bit clear, nonzero remaining fraction), preserves raw bytes, pushes Special, adds no flags including IE, and preserves PE/C0/C2/C3 under the unchanged all-masked/`FSW & 0x80df`/Empty/ten-byte/one-read profile. Minimum/middle/maximum payloads of both signs cover every TOP×FCW pair; bare and negative DB28/DFE0 FNSTSW fixtures run in the CLI. The narrow exact-DB28/budget-one/no-stop Unsupported exception uses positive pseudo-infinity; canonical SNaNs and pseudo-SNaNs do not match it. Native/JIT admission and physical acceptance remain closed. Mac mini/physical acceptance is deferred. See [S2-31ac CPU contract](specs/CPU_CONTRACT.md#local-s2-31ac-bounded-fld-m80fp-rax-db-28-canonical-signalingnans), [plan](plans/STAGE_2.md#s2-31ac-extend-bounded-fld-m80fp-rax-db-28-to-canonical-snans), [report](reports/STAGE_2_REPORT.md#s2-31ac--bounded-fld-m80fp-canonical-signalingnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ad evidence:** exact bare `DB 28` now also accepts pseudo-denormals (exponent zero, explicit integer bit set), preserving raw bytes/exponent zero, assigning Special, and adding no DE or other flags under the same all-masked/`FSW & 0x80df`/Empty/ten-byte/one-read gates. Signed zero/minimum/maximum fraction literals and exponent-one Normal controls cover TOP×FCW. Positive bare and negative DB28/DFE0 FNSTSW fixtures run through CLI. Pseudo-denormal encodings do not match the exact DB28/budget-one/no-stop pseudo-infinity Unsupported exception. Arithmetic consumption, unmasked exception delivery, native/JIT admission and physical acceptance remain outside scope; Mac mini/physical acceptance is deferred. See [S2-31ad CPU contract](specs/CPU_CONTRACT.md#local-s2-31ad-bounded-fld-m80fp-rax-db-28-pseudo-denormals), [plan](plans/STAGE_2.md#s2-31ad-extend-bounded-fld-m80fp-rax-db-28-to-pseudo-denormals), [report](reports/STAGE_2_REPORT.md#s2-31ad--bounded-fld-m80fp-pseudo-denormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **Следующий шаг:** дальнейшую конечную арифметику и architectural x87 effects выбирать отдельными срезами с независимыми vectors и доступным native state observation. S2-30n–p добавляют exact-only FMUL register forms; S2-30q–v add exact-only FDIV/FDIVR directions; S2-30w/x cover raw-copy FSTP/FST, S2-30y/z cover bounded FCHS/FABS sign-bit operations, S2-30aa covers bounded FTST comparison with zero, S2-30af covers bounded FCOMPP, S2-30ag/ah/ai cover bounded FUCOM/FUCOMP/FUCOMPP, and S2-30aj/ak/al add bounded FCOMI/FCOMIP/FUCOMI relation flags and definedness, with FCOMIP's physical TOP pop. These slices require future broader architectural coverage. Native x87 state observation remains unavailable. General rounding, special values, stack faults, exception delivery and CR0 behavior remain open. См. [локальный контракт S2-30y](specs/CPU_CONTRACT.md#local-s2-30y-bounded-fchs-d9-e0), [план S2-30y](plans/STAGE_2.md#s2-30y-bare-bounded-fchs-sign-bit-local-slice), [отчёт S2-30y](reports/STAGE_2_REPORT.md#s2-30y--bounded-fchs-sign-bit-slice), [локальный контракт S2-30z](specs/CPU_CONTRACT.md#local-s2-30z-bounded-fabs-d9-e1), [план S2-30z](plans/STAGE_2.md#s2-30z-bare-bounded-fabs-sign-bit-clear-local-slice), [отчёт S2-30z](reports/STAGE_2_REPORT.md#s2-30z--bounded-fabs-sign-clear-slice), [локальный контракт S2-30x](specs/CPU_CONTRACT.md#local-s2-30x-bounded-fst-sti-dd-d0i), [план S2-30x](plans/STAGE_2.md#s2-30x-bare-bounded-fst-sti-local-raw-copy-slice), [отчёт S2-30x](reports/STAGE_2_REPORT.md#s2-30x--bounded-fst-sti-local-raw-copy-slice), [локальный контракт S2-30v](specs/CPU_CONTRACT.md#local-s2-30v-exact-only-fdiv-sti-st0-dc-f8i), [план S2-30v](plans/STAGE_2.md#s2-30v-bare-exact-only-fdiv-sti-st0-bounded-local-slice), [отчёт S2-30v](reports/STAGE_2_REPORT.md#s2-30v--exact-only-fdiv-sti-st0-bounded-local-slice), and prior S2-30 slices. S2-29 and umbrella S2-28/S2-30 remain open.
- **S2-31ae evidence:** at this stage, exact bare `DD 00` accepted signed binary64 infinities, converting by integer fields to canonical binary80 Special with no new flags. The four FCWs, DD00-local `FSW & 0x80dd` gate, Empty slot, canonical48 Qword preflight/read-once, sticky DE/PE and push/pointer policy remain fixed. TOP×FCW signed literals, subnormal-then-infinity sticky status, QNaN/SNaN one-read rejection, bare and negative DFE0/FNSTSW fixtures, schema precision and native/JIT denial were covered. The Unsupported fixture then used QNaN; S2-31ag supersedes it by supporting canonical QNaNs and narrowing current rejection to SNaN. Unmasked exception delivery, physical behavior and native/JIT execution remain outside scope; binary80 subnormal behavior remains separately specified with no DE. The Linux x86 native-denial path is compile-checked only on the local arm64 host; `.serena` remains an existing layout-check exception. See [S2-31ae CPU contract](specs/CPU_CONTRACT.md#local-s2-31ae-bounded-fld-m64fp-rax-dd-00-signed-infinities), [plan](plans/STAGE_2.md#s2-31ae-extend-bounded-fld-m64fp-rax-dd-00-to-infinities), [report](reports/STAGE_2_REPORT.md#s2-31ae--bounded-fld-m64fp-signed-infinity-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31af evidence:** exact bare `D9 00` accepts signed binary32 infinities (exponent 255, zero fraction) after one Dword read and pushes canonical binary80 infinity with Special tag and no new flags. The four all-masked FCWs, D900-local `FSW & 0x80dd` gate, Empty destination, canonical48 preflight, sticky DE/PE and C1/TOP/pointer policy remain fixed. Signed TOP×FCW literals, subnormal-then-infinity sticky flags, one-read atomic QNaN/SNaN rejection, positive bare and negative D900/DFE0+FNSTSW fixtures, QNaN-only exact-D900 schema/CLI exception, JIT denial and native denial coverage are included. Unmasked exception delivery, physical behavior and native/JIT execution remain outside scope; binary80 m80 subnormals keep their separate no-DE behavior. The architecture-gated native-denial test compiles for Linux x86 but does not run on the local arm64 host; `.serena` remains the pre-existing layout-check exception. See [S2-31af CPU contract](specs/CPU_CONTRACT.md#local-s2-31af-bounded-fld-m32fp-rax-d9-00-signed-infinities), [plan](plans/STAGE_2.md#s2-31af-extend-bounded-fld-m32fp-rax-d9-00-to-infinities), [report](reports/STAGE_2_REPORT.md#s2-31af--bounded-fld-m32fp-signed-infinity-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ag evidence:** exact bare `DD 00` accepts canonical binary64 QNaNs (exponent `0x7ff`, quiet bit set, arbitrary payload) after one Qword read, converting integer fields to canonical binary80 with preserved sign/payload and Special tag. No status bits are added; sticky DE/PE and the existing four FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer and retirement behavior remain. Signed quiet-only, low/middle/max payload literals span TOP×FCW; subnormal→QNaN checks DE/PE stickiness; SNaN rejection is one-read and atomic. Positive bare and negative payload-bearing DD00/DFE0+FNSTSW fixtures run through CLI. The exact DD00/budget-one/no-stop schema rejection now admits canonical SNaN only. JIT/native remain denied and unmasked behavior is deferred; the local host did not execute the architecture-gated native runtime denial. See [S2-31ag CPU contract](specs/CPU_CONTRACT.md#local-s2-31ag-bounded-fld-m64fp-rax-dd-00-canonical-qnans), [plan](plans/STAGE_2.md#s2-31ag-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-qnans), [report](reports/STAGE_2_REPORT.md#s2-31ag--bounded-fld-m64fp-canonical-qnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ah evidence:** exact bare `D9 00` accepts canonical binary32 QNaNs (exponent `0xff`, quiet bit 22 set) of both signs and arbitrary lower payload, converting by integer fields to canonical binary80 with Special tag. No exception flags are added; sticky DE/PE and the four FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer and retirement policy remain. Quiet-only, low, middle and maximum payload literals span TOP×FCW; subnormal→QNaN checks sticky DE/PE; S2-31ah recorded one-read atomic SNaN rejection; S2-31aj adds masked SNaN support and changes the exact D900/budget-one/no-stop schema exception to an invalid-environment predicate. JIT/native remain denied and unmasked behavior remains deferred; Linux x86 native-denial compilation is checked but runtime denial is not executed on the local arm64 host. See [S2-31ah CPU contract](specs/CPU_CONTRACT.md#local-s2-31ah-bounded-fld-m32fp-rax-d9-00-canonical-qnans), [plan](plans/STAGE_2.md#s2-31ah-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-qnans), [report](reports/STAGE_2_REPORT.md#s2-31ah--bounded-fld-m32fp-canonical-qnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ai evidence:** exact bare `DD 00` accepts canonical binary64 SNaNs (exponent `0x7ff`, nonzero fraction, quiet bit clear) after one Qword read. Integer conversion sets binary80’s explicit integer and quiet bits, shifts payload by eleven, preserves sign, and assigns Special. Sticky IE alone is added; sticky DE/PE are preserved, and ES/SF/B or other flags are not introduced. Existing all-masked FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer/retirement behavior remain. Signed low/middle/max payload literals cover TOP×FCW; FSW/FNSTSW, DE/PE stickiness, memory/fault atomicity, pre-read IM=0 and pre-existing-IE rejection, subsequent-load rejection and FNCLEX recovery are covered. Bare positive and negative DD00/DFE0+FNSTSW fixtures run through CLI; JIT/native remain denied. The exact DD00/budget-one/no-stop schema exception now represents an invalid FCW or status gate, independent of operand class. Unmasked behavior and physical acceptance remain outside scope. See [S2-31ai CPU contract](specs/CPU_CONTRACT.md#local-s2-31ai-bounded-fld-m64fp-rax-dd-00-canonical-snans), [plan](plans/STAGE_2.md#s2-31ai-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-snans), [report](reports/STAGE_2_REPORT.md#s2-31ai--bounded-fld-m64fp-canonical-snan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31aj evidence:** exact bare `D9 00` accepts canonical binary32 SNaNs (exponent `0xff`, nonzero fraction, quiet bit 22 clear) after one Dword read. Integer conversion sets binary80’s explicit integer and quiet bits, shifts payload by 40, preserves sign, and assigns Special. Sticky IE alone is added; sticky DE/PE and other admitted flags remain unchanged. Existing all-masked FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer/retirement behavior remain. Signed low/middle/max payload literals cover TOP×FCW; clean TOP=0 FSW is `0x3801`; SNaN→quieted-QNaN, DE/PE stickiness, memory faults, pre-read unmasked-FCW/pre-existing-IE gates, subsequent-load rejection, and FNCLEX recovery are covered. Bare positive and negative D900/DFE0+FNSTSW fixtures run through CLI; native/JIT remain denied. The exact D900/budget-one/no-stop schema exception requires an invalid FCW or status bit in `0x80dd`, independent of operand class. Unmasked delivery and physical acceptance remain outside scope. See [S2-31aj CPU contract](specs/CPU_CONTRACT.md#local-s2-31aj-bounded-fld-m32fp-rax-d9-00-canonical-snans), [plan](plans/STAGE_2.md#s2-31aj-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-snans), [report](reports/STAGE_2_REPORT.md#s2-31aj--bounded-fld-m32fp-canonical-snan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ak evidence:** exact D900/DD00 alone now admits all IE/DE/PE sticky combinations by changing the local mask from `0x80dd` to `0x80dc`; ZE/OE/UE/SF/ES/B still reject before read. The full sticky-combination matrix spans source classes, FCWs and TOP, with same-width/cross-width sequences, repeated SNaNs, second-load fault atomicity, FNCLEX and unchanged FILD/m80 gates. Exact D900/DD00 budget-one/no-stop schema predicates now use forbidden status rather than source-class exceptions; existing CLI/FNSTSW SNaN fixtures observe IE. Native/JIT and unmasked delivery remain outside the contract, so ISSUE-019 stays open. See [S2-31ak CPU contract](specs/CPU_CONTRACT.md#local-s2-31ak-sticky-exception-flags-for-bounded-m32m64-fld), [plan](plans/STAGE_2.md#s2-31ak-preserve-sticky-ie-de-pe-across-exact-m32m64-fld-loads), [report](reports/STAGE_2_REPORT.md#s2-31ak--sticky-ie-de-pe-across-bounded-m32m64-fld), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31al evidence:** exact D910/D918 now also store signed binary80 Zero when physical TOP is tagged Zero and raw exponent/significand are canonical zero. The sign maps to binary32 `+0`/`-0`; existing FCW/`FSW & 0x80df` gates and atomic Dword write remain. FST preserves stack state; FSTP changes TOP/tag only after successful store. Tests cover TOP×FCW×sign×PE/C1, FLD-zero compositions, mismatched encodings, backend/#PF and range atomicity, literal CLI/FNSTSW fixtures and native/JIT denial. General x87 special arithmetic and physical behavior remain open under ISSUE-019. See [S2-31al CPU contract](specs/CPU_CONTRACT.md#local-s2-31al-signed-zero-fst-m32fp-and-fstp-m32fp), [plan](plans/STAGE_2.md#s2-31al-store-signed-binary80-zero-through-bounded-m32-fstfstp), [report](reports/STAGE_2_REPORT.md#s2-31al--signed-zero-stores-for-bounded-m32-fstfstp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31am evidence:** exact DD10/DD18 now store signed canonical binary80 Zero as binary64 `+0`/`-0`, requiring both the physical Zero tag and zero exponent/significand. The existing store FCW/`FSW & 0x80df` gates remain; one atomic Qword write precedes any FSTP pop, with no data read. TOP×FCW×sign×PE/C1 matrices, cross-width FLD-zero compositions, status/source rejection, write faults, canonical boundary checks, literal CLI/FNSTSW fixtures, and native/JIT denial are covered. Full x87 special arithmetic and physical behavior remain open under ISSUE-019. See [S2-31am CPU contract](specs/CPU_CONTRACT.md#local-s2-31am-signed-zero-fst-m64fp-and-fstp-m64fp), [plan](plans/STAGE_2.md#s2-31am-store-signed-binary80-zero-through-bounded-m64-fstfstp), [report](reports/STAGE_2_REPORT.md#s2-31am--signed-zero-stores-for-bounded-m64-fstfstp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31an evidence:** exact DB38 FSTP m80fp now also stores raw signed canonical binary80 zero when physical TOP has the matching Zero tag; all ten bytes are preserved. The existing FCW/`FSW & 0x80df` gate, full-range preflight, one atomic write/no read, and post-success pop remain. Normal sources are unchanged. TOP×FCW×sign×PE/C1, canaries, mismatched source rejection, literal CLI/FNSTSW fixtures, and native/JIT denial are covered. General x87 special arithmetic and physical behavior remain open under ISSUE-019. See [S2-31an CPU contract](specs/CPU_CONTRACT.md#local-s2-31an-signed-zero-fstp-m80fp-rax-db-38), [plan](plans/STAGE_2.md#s2-31an-store-canonical-signed-binary80-zero-through-bounded-fstp-m80fp), [report](reports/STAGE_2_REPORT.md#s2-31an--signed-zero-binary80-stores-through-bounded-fstp-m80fp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ao evidence:** exact DB38 FSTP m80fp also stores canonical signed binary80 infinity with Special tag, preserving its raw ten-byte representation. Existing Valid+Normal and Zero+Zero pairs, four FCWs, `FSW & 0x80df`, no-read one-write, post-success pop, and pointer/retirement policy remain. Pseudo-infinity, NaNs, finite special encodings, and mismatched tags reject before access. TOP×FCW×sign×PE/C1, cross-width infinity loads, memory/range atomicity, CLI/FNSTSW and native/JIT denial are covered. General x87 exception delivery and physical behavior remain open under ISSUE-019. See [S2-31ao CPU contract](specs/CPU_CONTRACT.md#local-s2-31ao-signed-infinity-fstp-m80fp-rax-db-38), [plan](plans/STAGE_2.md#s2-31ao-store-canonical-signed-binary80-infinity-through-bounded-fstp-m80fp), [report](reports/STAGE_2_REPORT.md#s2-31ao--signed-infinity-binary80-store-through-bounded-fstp-m80fp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31ap evidence:** exact DD10/DD18 m64 stores now accept canonical signed
  binary80 infinity only with physical Special tag, emitting literal binary64
  infinity without new flags. Existing Normal/zero paths and their gates remain;
  Qword store is one-write/no-read, with FSTP pop after success. Matrices,
  all-width load/store sequences, malformed source/status rejection, atomicity,
  CLI/FNSTSW and native/JIT denial are covered. Broader x87 exception delivery
  and physical behavior remain open under ISSUE-019. See [S2-31ap CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31ap-signed-infinity-fstfstp-m64fp-rax),
  [plan](plans/STAGE_2.md#s2-31ap-store-canonical-signed-binary80-infinity-through-bounded-m64-fstfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31ap--signed-infinity-stores-for-bounded-m64-fstfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ax evidence (historical gate, superseded by S2-31ay):** exact D910/D918 m32 stores now convert Special-tagged
  canonical binary80 SNaNs to quiet binary32 values, preserving sign and the
  upper 22 payload bits while truncating lower 40 without PE. Sticky IE is
  committed only after the atomic Dword write succeeds; source bytes and sticky
  PE remain unchanged, and D918 pops only after success. At the S2-31ax stage,
  the store gate blocked subsequent stores until FNCLEX cleared IE; S2-31ay
  now admits sticky IE for the exact m32/m64 FST/FSTP conversion helpers.
  Native runtime and physical
  acceptance remain unverified and deferred to the final combined S2-31
  substage under ISSUE-019. See [S2-31ax CPU contract](specs/CPU_CONTRACT.md#local-s2-31ax-canonical-signalingnan-fstfstp-m32fp-rax-d9-10-d9-18),
  [plan](plans/STAGE_2.md#s2-31ax-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m32fp),
  [report](reports/STAGE_2_REPORT.md#s2-31ax--canonical-signalingnan-binary80-to-binary32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ay evidence:** exact D910/D918 and DD10/DD18 conversion helpers now
  admit masked sticky IE by changing only their local FSW mask to `0x80de`.
  ZE/DE/OE/UE/SF/ES/B, FCW restrictions, source class/tag rules, canonical range
  checks, and all other x87 instruction gates remain unchanged. Incoming IE/PE
  is preserved; successful SNaN writes OR IE and inexact finite writes OR PE
  only after the atomic store. Repeated stores remain available with IE set;
  FNCLEX can still clear status. Full validation, CLI fixture reachability, and
  independent review are recorded in the [S2-31ay report](reports/STAGE_2_REPORT.md#s2-31ay--sticky-ie-m32-m64-stores).
  Native runtime and physical acceptance remain deferred to the final combined
  S2-31 substage. See the [CPU contract](specs/CPU_CONTRACT.md#local-s2-31ay-admit-sticky-ie-fstfstp-m32-m64),
  [plan](plans/STAGE_2.md#s2-31ay-admit-sticky-ie-in-bounded-m32-m64-fst-fstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31aq evidence:** exact D910/D918 now store Special-tagged canonical
  binary80 infinity as signed binary32 infinity using integer fields. The
  existing FCW/FSW gates, one-write/no-read Dword access, and post-success FSTP
  pop remain. Normal/zero paths and native/JIT denial are unchanged. Full
  general x87 exception delivery and physical behavior remain open under
  ISSUE-019; the final combined S2-31 substage owns physical acceptance. See
  [S2-31aq CPU contract](specs/CPU_CONTRACT.md#local-s2-31aq-signed-infinity-fstfstp-m32fp-rax),
  [plan](plans/STAGE_2.md#s2-31aq-store-canonical-signed-binary80-infinity-through-bounded-m32-fstfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31aq--signed-infinity-stores-for-bounded-m32-fstfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ar evidence:** exact DB38 now also stores canonical binary80 QuietNaN
  with a Special-tagged TOP, preserving the full ten-byte sign/payload without
  new flags. Existing Normal, Zero and Infinity pairs, the four-FCW/FSW gate,
  one-write/no-read access and pop-after-success remain. SNaN and unsupported
  encodings reject before access. General x87 exception delivery and physical
  behavior remain open under ISSUE-019; physical acceptance belongs to the
  combined final S2-31 substage. See [S2-31ar CPU contract](specs/CPU_CONTRACT.md#local-s2-31ar-canonical-quietnan-fstp-m80fp-rax-db-38),
  [plan](plans/STAGE_2.md#s2-31ar-store-canonical-binary80-quietnan-through-bounded-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31ar--canonical-quietnan-binary80-store-through-bounded-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31as evidence:** exact DB38 now also raw-stores canonical binary80
  SignalingNaN from a Special-tagged TOP, preserving the signaling bit and
  payload without setting IE. Existing status gates, one-write/no-read access,
  and pop-after-success remain. General x87 exception delivery and physical
  behavior remain open under ISSUE-019; physical acceptance belongs to the
  combined final S2-31 substage. See [S2-31as CPU contract](specs/CPU_CONTRACT.md#local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38),
  [plan](plans/STAGE_2.md#s2-31as-store-canonical-binary80-signalingnan-through-bounded-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31as--canonical-signalingnan-binary80-store-through-bounded-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31at evidence:** exact DB38 now also raw-stores canonical binary80
  subnormals from a Special-tagged TOP, preserving sign and all fraction bits
  without new DE/UE/PE. Existing status gates, one-write/no-read access, and
  pop-after-success remain. General x87 exception delivery and physical
  behavior remain open under ISSUE-019; physical acceptance belongs to the
  combined final S2-31 substage. See [S2-31at CPU contract](specs/CPU_CONTRACT.md#local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38),
  [plan](plans/STAGE_2.md#s2-31at-store-canonical-binary80-subnormal-through-bounded-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31at--canonical-subnormal-binary80-store-through-bounded-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31au evidence:** exact DD10/DD18 m64 stores also convert a Special-tagged
  canonical binary80 QuietNaN to binary64 by preserving sign and the upper 52
  fraction bits, truncating the lower 11 without new flags. Existing gates and
  atomic write/pop behavior remain. General x87 exception delivery, native
  runtime admission and physical behavior remain open under ISSUE-019; physical
  acceptance belongs to the final combined S2-31 substage. See [S2-31au CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31au-canonical-quietnan-fstfstp-m64fp-rax-dd-10-dd-18),
  [plan](plans/STAGE_2.md#s2-31au-store-canonical-binary80-quietnan-through-bounded-fstfstp-m64fp),
  [report](reports/STAGE_2_REPORT.md#s2-31au--canonical-quietnan-binary80-to-binary64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31av evidence:** exact D910/D918 m32 stores now convert Special-tagged
  canonical binary80 QuietNaNs to binary32, preserving sign and upper 23 payload
  bits while truncating lower 40 without rounding or new flags. Existing gates and
  atomic write/pop behavior remain. General x87 exception delivery, native
  runtime admission and physical behavior remain open under ISSUE-019; physical
  acceptance belongs to the final combined S2-31 substage. See [S2-31av CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31av-canonical-quietnan-fstfstp-m32fp-rax-d9-10-d9-18),
  [plan](plans/STAGE_2.md#s2-31av-store-canonical-binary80-quietnan-through-bounded-fstfstp-m32fp),
  [report](reports/STAGE_2_REPORT.md#s2-31av--canonical-quietnan-binary80-to-binary32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31aw evidence:** exact DD10/DD18 m64 stores now accept canonical
  Special-tagged binary80 SNaNs, quiet the binary64 destination, and add sticky
  IE only after a successful atomic write. The upper 51 payload bits and sign
  survive, the lower 11 truncate without PE, and the raw source remains
  unchanged; DD18 pops only after success. FSW gates, PE, C1/TOP behavior and
  ES/B/SF policy are covered along with FNCLEX recovery; native runtime and physical
  acceptance remain unverified and deferred to the final combined S2-31
  substage. See [S2-31aw CPU contract](specs/CPU_CONTRACT.md#local-s2-31aw-canonical-signalingnan-fstfstp-m64fp-rax-dd-10-dd-18),
  [plan](plans/STAGE_2.md#s2-31aw-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m64fp),
  [report](reports/STAGE_2_REPORT.md#s2-31aw--canonical-signalingnan-binary80-to-binary64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31az evidence:** exact D910/D918 and DD10/DD18 store helpers now admit
  sticky masked DE together with IE/PE by changing only their local FSW mask to
  `0x80dc`. ZE/OE/UE/SF/ES/B, FCWs, classes/tags, finite-result limits and other
  x87 gates remain unchanged. Incoming IE/DE/PE is preserved; SNaN adds IE and
  inexact finite conversion adds PE only after the atomic write, with no new DE.
  Cross-width binary32-subnormal FLD→binary64 FST/FSTP fixtures pass through
  interpreter; explicit JIT-denial checks pass, and native-admission denial is
  covered by an x86-target unit test that compiled but could not run on this
  arm64 host. The full offline workspace suite passed 775 tests and workspace
  plus Linux x86_64 runner checks passed. Independent Sol-medium review found no blocking defects.
  Native runtime and physical acceptance remain deferred to the final combined
  S2-31 substage under ISSUE-019. See [S2-31az CPU contract](specs/CPU_CONTRACT.md#local-s2-31az-admit-sticky-de-m32-m64-fstfstp),
  [plan](plans/STAGE_2.md#s2-31az-admit-sticky-de-in-bounded-m32-m64-fst-fstp),
  [report](reports/STAGE_2_REPORT.md#s2-31az--sticky-de-in-bounded-m32-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ba evidence:** exact DB38 raw FSTP now admits pre-existing masked IE
  and DE alongside PE by changing only its local gate to `0x80dc`. It preserves
  source bytes and incoming IE/DE/PE, generates no status flags, clears C1,
  preserves C0/C2/C3, and changes TOP/tag only after its one atomic ten-byte
  write. The existing source/tag pairs and all FCW/address checks remain.
  Cross-width subnormal/SNaN FLD compositions, raw-SNaN narrow-FST composition,
  all eight sticky combinations, forbidden controls, fault atomicity, CLI,
  JIT-denial and x86-target native-denial coverage are included. Native runtime
  is unexecuted on this arm64 host and physical acceptance remains deferred to
  the final combined S2-31 substage under ISSUE-019. See [S2-31ba CPU contract](specs/CPU_CONTRACT.md#local-s2-31ba-sticky-ie-de-raw-fstp-m80fp-db-38),
  [plan](plans/STAGE_2.md#s2-31ba-admit-sticky-ie-de-to-raw-binary80-fstp),
  [report](reports/STAGE_2_REPORT.md#s2-31ba--sticky-ie-de-on-raw-binary80-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bb evidence:** exact D910/D918 finite m32 stores now admit canonical
  Normal binary80 sources through unbiased exponent 127. Existing integer
  rounding normalizes carry before the final range check: 126→127 is supported,
  while 127→128 and an exponent-128 input reject before data access and do not
  set OE. Sticky IE/DE/PE and all other gates remain unchanged; subnormal store
  results remain Unsupported. Separate signed max-finite FLD and store fixtures
  exercise exponent 127 through interpreter/schema, with JIT/native denial
  coverage.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [S2-31bb CPU contract](specs/CPU_CONTRACT.md#local-s2-31bb-finite-exponent-127-bounded-m32-stores),
  [plan](plans/STAGE_2.md#s2-31bb-finite-exponent-127-bounded-m32-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bb--finite-exponent-127-bounded-m32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bc evidence:** exact DD10/DD18 finite m64 stores now admit canonical
  Normal binary80 sources through unbiased exponent 1023. Carry is normalized
  before the final range check: 1022→1023 is supported, while 1023→1024 and a
  source at exponent 1024 reject before access without setting OE. Subnormal
  store results, minimum exponent and all existing gates remain unchanged.
  Separate signed max-finite FLD and store fixtures exercise the interpreter
  path, with JIT/native denial coverage. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [S2-31bc CPU contract](specs/CPU_CONTRACT.md#local-s2-31bc-finite-exponent-1023-bounded-m64-stores),
  [plan](plans/STAGE_2.md#s2-31bc-finite-exponent-1023-bounded-m64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bc--finite-exponent-1023-bounded-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bd evidence:** exact D910/D918 stores now accept only exactly
  representable binary32 subnormal results from canonical Normal binary80
  sources with unbiased exponents -149 through -127. Integer downshift checks
  all discarded bits; inexact tiny results and sources below -149 remain
  Unsupported before access. Sticky flags are retained without generating DE,
  UE or PE. Boundary, FLD→store, write/range atomicity, CLI and JIT/native
  denial coverage is included. Native runtime and physical acceptance remain
  deferred under ISSUE-019. See [S2-31bd CPU contract](specs/CPU_CONTRACT.md#local-s2-31bd-exact-binary32-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31bd-exact-binary32-subnormal-results-through-m32-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bd--exact-binary32-subnormal-results-through-m32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31be evidence:** exact DD10/DD18 stores now admit only representable
  binary64 subnormal results from canonical Normal binary80 sources with
  exponents -1074 through -1023. Integer downshift rejects nonzero discarded
  bits and values below the minimum subnormal before data access. Sticky flags
  remain unchanged and no DE/UE/PE is generated. Boundary, FLD→store,
  write/range atomicity, CLI and JIT/native denial coverage is included.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [S2-31be CPU contract](specs/CPU_CONTRACT.md#local-s2-31be-exact-binary64-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31be-exact-binary64-subnormal-results-through-m64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31be--exact-binary64-subnormal-results-through-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bf evidence:** masked inexact D910/D918 stores round only canonical
  Normal binary80 sources in the binary32 subnormal exponent range. They accept
  nonzero subnormal results, reject zero/minimum-normal promotion before access,
  and defer sticky UE+PE with C1 magnitude-increment state until after the
  atomic Dword write. Exact behavior is unchanged; no DE is added. Tie, shift,
  TOP/FCW/sticky-flag matrices, FNSTSW/FNCLEX, atomicity, CLI and JIT/native
  denial coverage is included. Native runtime and physical acceptance remain
  deferred under ISSUE-019. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31bf-masked-inexact-binary32-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31bf-masked-inexact-binary32-subnormal-results-through-m32-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bf--masked-inexact-binary32-subnormal-results-through-m32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bg evidence:** masked inexact DD10/DD18 stores now round canonical
  Normal binary80 sources with exponents -1074 through -1023. They accept only
  nonzero binary64 subnormal fractions, reject below-range and promotion cases
  before access, and defer sticky UE+PE with C1 magnitude-increment state until
  after atomic Qword storage; exact behavior is unchanged and no DE is added.
  TOP/opcode/FCW/sign/sticky matrices, every shift, upper-bound checks,
  FNSTSW/FNCLEX, the then-current UE gate (superseded by S2-31bh),
  write/range atomicity, CLI and JIT/native denial are covered. Native runtime
  and physical acceptance remain deferred under this
  open issue. See [S2-31bg CPU contract](specs/CPU_CONTRACT.md#local-s2-31bg-masked-inexact-binary64-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31bg-masked-inexact-binary64-subnormal-results-through-m64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bg--masked-inexact-binary64-subnormal-results-through-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31bh evidence:** the m32/m64 FST/FSTP gates now admit incoming masked UE
  with IE/DE/PE while continuing to reject ZE/OE/SF/ES/B. UE alone does not
  synthesize PE, and any conversion-generated flags or FSTP state changes commit
  only after the atomic store. All 16 sticky combinations, repeated underflow,
  cross-width exact storage from the unchanged source, FNSTSW/FNCLEX, fault
  atomicity, CLI, and JIT/native denial are covered. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bh-sticky-ue-re-entry-for-bounded-m32m64-stores),
  [plan](plans/STAGE_2.md#s2-31bh-admit-sticky-ue-in-bounded-binary32binary64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bh--sticky-ue-reentry-for-bounded-m32-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31bi evidence:** raw DB38 FSTP m80fp now admits incoming masked UE with
  IE/DE/PE under the narrowed `0x80cc` gate while retaining the existing source
  classes/tags, ten-byte payload and no-generated-flags policy. UE-only preserves
  a signaling NaN without adding IE/PE, normalizing or quieting it. All 16 sticky
  combinations, cross-width underflow-to-raw-store composition, FNSTSW/FNCLEX,
  write/range atomicity, CLI, and JIT/native denial are covered. Native runtime
  and physical acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bi-sticky-ue-on-raw-binary80-fstp-m80fp),
  [plan](plans/STAGE_2.md#s2-31bi-admit-sticky-ue-in-raw-binary80-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31bi--sticky-ue-on-raw-binary80-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bj evidence:** bounded D9 10/D9 18 now round canonical binary80
  exponents -16382..=-150 directly to signed zero or minimum binary32 subnormal,
  using explicit half comparison at -150 and avoiding oversized shifts.
  Inexact UE+PE and C1 commit only after atomic storage. CLI/JIT/native denial and
  ISA matrix coverage are documented in [contract](specs/CPU_CONTRACT.md#local-s2-31bj-extreme-binary32-underflow-to-zero-or-minimum-subnormal),
  [plan](plans/STAGE_2.md#s2-31bj-round-extreme-underflow-through-binary32-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bj--extreme-binary32-underflow-to-zero-or-minimum-subnormal).
  Native runtime and physical acceptance remain deferred under ISSUE-019.

- **S2-31bk evidence:** bounded DD 10/DD 18 now round canonical binary80
  exponents -16382..=-1075 directly to signed zero or minimum binary64 subnormal,
  using an explicit half comparison at -1075 and avoiding oversized shifts.
  Inexact UE+PE and C1 commit only after atomic storage. CLI/JIT/native denial and
  ISA matrix coverage are documented in [contract](specs/CPU_CONTRACT.md#local-s2-31bk-extreme-binary64-underflow-to-zero-or-minimum-subnormal),
  [plan](plans/STAGE_2.md#s2-31bk-round-extreme-underflow-through-binary64-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bk--extreme-binary64-underflow-to-zero-or-minimum-subnormal).
  Native runtime and physical acceptance remain deferred under ISSUE-019.

- **S2-31bl evidence:** bounded D9 10/D9 18 now round Special-tagged canonical
  binary80 subnormal sources to signed zero or signed minimum binary32 subnormal
  using the existing RC. All admitted results commit UE+PE after atomic storage;
  C1 tracks minimum-subnormal selection. ISA matrix, CLI fixture and JIT/native
  denial are documented in [contract](specs/CPU_CONTRACT.md#local-s2-31bl-binary80-subnormal-to-binary32-underflow-rounding),
  [plan](plans/STAGE_2.md#s2-31bl-round-binary80-subnormals-through-binary32-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bl--binary80-subnormal-to-binary32-underflow-rounding).
  Native runtime and physical acceptance remain deferred under ISSUE-019.
- **S2-31bm evidence:** bounded DD10/DD18 now accept Special-tagged canonical
  binary80 subnormal sources and round them directly to signed zero or minimum
  binary64 subnormal. UE+PE and C1 commit only after successful Qword storage;
  no DE is added, and FSTP pop/status/pointers remain atomic. Four payloads,
  signs, RCs, TOPs, sticky flags, invalid encodings/gates, FNSTSW/FNCLEX,
  raw-m80 preservation, CLI fixtures and JIT denial are covered. Native runtime
  and physical x86 acceptance remain unverified under open ISSUE-019. See the
  [S2-31bm CPU contract](specs/CPU_CONTRACT.md#local-s2-31bm-binary80-subnormal-to-binary64-underflow-rounding),
  [plan](plans/STAGE_2.md#s2-31bm-round-binary80-subnormals-through-binary64-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bm--binary80-subnormal-to-binary64-underflow-rounding).
- **S2-31bn evidence:** bounded D910/D918 now admit only exponent -127 Normal
  sources whose direct `S/2^41` rounding selects binary32 minimum normal. PE+C1
  follow the destination rounding; UE is independently decided by rounded
  `S/2^40`, with sticky UE preserved when no new underflow is indicated. The
  threshold oracle covers signed RC neighbors, the separate 24-bit UE boundary,
  all TOPs/sticky IE/DE/UE/PE states, both opcodes, atomic Dword failures,
  FNSTSW/FNCLEX and interpreter CLI fixtures; JIT/native admission remain
  closed. Native runtime and physical x86 acceptance remain unverified under
  open ISSUE-019. See the [S2-31bn CPU contract](specs/CPU_CONTRACT.md#local-s2-31bn-binary80-exponent--127-to-binary32-minimum-normal),
  [plan](plans/STAGE_2.md#s2-31bn-round-exponent--127-binary80-normals-directly-to-binary32-minimum-normal),
  and [report](reports/STAGE_2_REPORT.md#s2-31bn--binary80-normal-to-binary32-minimum-normal-rounding).
- **S2-31bo evidence:** bounded DD10/DD18 admit exponent -1023 Normal sources
  only when direct `S/2^12` rounding selects binary64 minimum normal. PE+C1
  follow that destination result; UE is independently decided by rounding the
  unbounded 53-bit significand from `S>>11`. Threshold/oracle coverage spans
  signed RC neighbors, the separate UE boundary, all TOPs/sticky IE/DE/UE/PE
  combinations, both opcodes, atomic Qword failures, FNSTSW/FNCLEX and CLI
  fixtures; JIT/native admission remain closed. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See the [CPU contract](specs/CPU_CONTRACT.md#local-s2-31bo-binary80-exponent--1023-to-binary64-minimum-normal),
  [plan](plans/STAGE_2.md#s2-31bo-round-exponent--1023-binary80-normals-directly-to-binary64-minimum-normal),
  and [report](reports/STAGE_2_REPORT.md#s2-31bo--binary80-normal-to-binary64-minimum-normal-rounding).
- **S2-31bp evidence:** bounded D910/D918 exponent-127 Normal inputs whose
  sign-aware rounding carries `S/2^40` to `1<<24` now store signed binary32
  infinity and commit sticky OE+PE with C1 after the Dword write. The test
  matrix covers nearest/directed thresholds and neighbors, both signs, RCs,
  opcodes, TOPs and sticky IE/DE/UE/PE combinations; exponent-128 sources
  still reject. Atomicity, FNSTSW/FNCLEX, sticky-OE re-entry, CLI and JIT/native
  denial are covered. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See the [contract](specs/CPU_CONTRACT.md#local-s2-31bp-binary80-exponent-127-overflow-to-binary32-infinity),
  [plan](plans/STAGE_2.md#s2-31bp-round-exponent-127-binary80-normals-to-binary32-infinity-on-overflow),
  and [report](reports/STAGE_2_REPORT.md#s2-31bp--binary80-normal-to-binary32-infinity-on-overflow).

- **S2-31bq evidence:** bounded DD10/DD18 exponent-1023 Normal values that
  round beyond maximum finite binary64 now store signed infinity and commit
  OE+PE+C1 after a successful Qword write. Exact source vectors cover nearest
  and directed thresholds, signs, RCs, both opcodes/TOPs and all 16 sticky
  IE/DE/UE/PE combinations; exponent-1024 still rejects. Atomicity, FNSTSW,
  FNCLEX, CLI fixtures and JIT/native denial are covered. Native runtime and
  physical acceptance remain deferred under ISSUE-019. See the [contract](specs/CPU_CONTRACT.md#local-s2-31bq-binary80-exponent-1023-overflow-to-binary64-infinity),
  [plan](plans/STAGE_2.md#s2-31bq-round-exponent-1023-binary80-normals-to-binary64-infinity-on-overflow),
  and [report](reports/STAGE_2_REPORT.md#s2-31bq--binary80-normal-to-binary64-infinity-on-overflow).

- **S2-31br evidence:** bounded D910/D918 and DD10/DD18 stores now admit all
  prior sticky IE/DE/OE/UE/PE combinations while ZE/SF/ES/B remain rejected.
  Conversion-generated flags and C1 are recomputed after a successful write;
  OE alone is preserved without adding PE. ISA matrices, overflow re-entry,
  FNSTSW/FNCLEX, atomicity and CLI/JIT/native-denial fixture checks cover this
  slice. Native runtime and physical acceptance remain deferred under
  ISSUE-019. See the [contract](specs/CPU_CONTRACT.md#local-s2-31br-sticky-status-admission-for-bounded-m32m64-fstfstp),
  [plan](plans/STAGE_2.md#s2-31br-admit-sticky-status-on-bounded-m32m64-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31br--sticky-status-admission-for-bounded-m32m64-stores).

- **S2-31bs evidence:** exact DB38 raw FSTP m80fp now admits incoming sticky OE
  with IE/DE/UE/PE under `FSW & 0x80c4`, while ZE/SF/ES/B remain rejected.
  The write preserves all ten source bytes and generates no flags; C1 clearing,
  status, pointers and pop remain atomic after the single write. Coverage spans
  all 32 sticky combinations, cross-width overflow-to-raw-store composition,
  OE-only SNaN/CLI, FNSTSW/FNCLEX, fault/range atomicity, and JIT/native denial.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31bs-sticky-oe-on-raw-binary80-fstp-m80fp),
  [plan](plans/STAGE_2.md#s2-31bs-admit-sticky-oe-to-raw-binary80-fstp),
  and [report](reports/STAGE_2_REPORT.md#s2-31bs--sticky-oe-on-raw-binary80-fstp-m80fp).

- **S2-31bt evidence:** exact `DB 10`/`DB 18` m32 integer stores admit only
  Zero-tagged canonical binary80 zero with exponent/sign word `0x0000` or
  `0x8000` and zero significand. Both signs store integer zero under all RC
  modes, preserve PE and source payloads, and clear C1 after the atomic Dword
  write. FIST preserves tag/TOP; FISTP pops only after write success. Tests
  cover sign/RC/opcode/TOP/PE/C1, source and control rejection, and write/range
  atomicity. CLI fixtures observe each opcode with FNSTSW; JIT/native denial
  remain explicit. Native runtime and physical acceptance are deferred under
  ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bt-exact-signed-zero-fist-m32int-rax-db-10-db-18),
  [plan](plans/STAGE_2.md#s2-31bt-exact-signed-zero-fist-m32int), and
  [report](reports/STAGE_2_REPORT.md#s2-31bt--exact-signed-zero-fist-m32int).

- **S2-31bu evidence:** exact `DF 10`/`DF 18` m16 integer stores admit only
  Zero-tagged canonical binary80 zeros, including both signs. They store Word
  zero under every admitted RC, preserve PE and raw source bytes, and clear C1
  after the atomic Word write. FIST retains tag/TOP; FISTP pops only after
  success. Tests cover sign/RC/opcode/TOP/PE/C1, unsupported tags/payloads,
  FCW/FSW/prefix rejection, and write/range atomicity. A CLI/FNSTSW fixture is
  registered for JIT/native denial. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bu-exact-signed-zero-fist-m16int-rax-df-10-df-18),
  [plan](plans/STAGE_2.md#s2-31bu-exact-signed-zero-fist-m16int), and
  [report](reports/STAGE_2_REPORT.md#s2-31bu--exact-signed-zero-fist-m16int).

- **S2-31bv evidence:** exact `DF 38` FISTP m64 integer stores admit only
  Zero-tagged canonical binary80 zero with sign/exponent word `0x0000` or
  `0x8000` and zero significand. Both signs store Qword zero under all admitted
  RC modes, preserve PE and source bytes, and clear C1 after the atomic write.
  FISTP pops only after write success. ISA tests cover sign/RC/TOP/PE/C1, source
  and control rejection, and write/range atomicity; a CLI/FNSTSW fixture is
  registered for JIT/native denial. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bv-exact-signed-zero-fistp-m64int-rax-df-38),
  [plan](plans/STAGE_2.md#s2-31bv-exact-signed-zero-fistp-m64int), and
  [report](reports/STAGE_2_REPORT.md#s2-31bv--exact-signed-zero-fistp-m64int).

- **S2-31bw evidence:** bare `DB 10`/`DB 18` now round Valid-tagged canonical
  binary80 Normal fractions with unbiased exponent -16382 through -1 to signed
  integer zero or one using the four existing RC modes. All admitted fractions
  add sticky PE; C1 is set iff magnitude one is selected. Comparisons handle
  the half threshold without exponent-sized shifts. ISA coverage crosses sign,
  RC, opcode, TOP, PE/C1, threshold neighbors and extreme exponents; fault/range
  atomicity, CLI/FNSTSW, and JIT/native denial are covered. Native runtime and
  physical acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bw-round-normal-binary80-fractions-m32-fist),
  [plan](plans/STAGE_2.md#s2-31bw-round-binary80-fractions-through-bounded-m32-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31bw--round-binary80-fractions-through-m32-fistfstp).

- **S2-31bx evidence:** bare `DF 10`/`DF 18` now round Valid-tagged canonical
  binary80 Normal fractions with unbiased exponent -16382 through -1 to Word
  zero or signed one, using the four existing RC modes and the same
  comparison-only threshold rule as m32. Each accepted fraction adds sticky
  PE; C1 reflects whether magnitude one was selected. ISA coverage crosses
  sign/RC/opcode/TOP/PE/C1, half neighbors and extreme exponents; class/gate
  rejection, write/range atomicity, CLI/FNSTSW and JIT/native denial are covered.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31bx-round-normal-binary80-fractions-m16-fist),
  [plan](plans/STAGE_2.md#s2-31bx-round-binary80-fractions-through-bounded-m16-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31bx--round-binary80-fractions-through-m16-fistfstp).

- **S2-31by evidence:** bare `DF 38` now rounds Valid-tagged canonical
  binary80 Normal fractions with unbiased exponent -16382 through -1 to Qword
  zero or signed one. The existing comparison-only rule adds sticky PE for
  every admitted fraction and sets C1 iff magnitude one was selected, with no
  new IE/DE/OE/UE. Coverage includes signs/RC/TOP/PE/C1, threshold neighbors,
  extreme exponents, rejection controls, Qword canaries, write/range atomicity,
  CLI/FNSTSW, and JIT/native denial. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31by-round-normal-binary80-fractions-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31by-round-normal-binary80-fractions-through-bounded-m64-fistp),
  and [report](reports/STAGE_2_REPORT.md#s2-31by--round-binary80-fractions-through-m64-fistp).

- **S2-31bz evidence:** bare `DB 10`/`DB 18` now convert Special-tagged
  canonical binary80 ±Infinity to Dword integer indefinite `0x80000000` under
  the admitted masked FCWs. After successful write, IE is sticky, C1 clears,
  incoming PE and C0/C2/C3 are preserved, and no other exception status is
  added. Tests cover signs/FCWs/opcodes/TOP/PE/C1, source/control rejection,
  finite -2^31 distinction, write/range atomicity, CLI/FNSTSW/FNCLEX, and
  JIT/native denial. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bz-masked-canonical-infinity-fist-m32int),
  [plan](plans/STAGE_2.md#s2-31bz-masked-binary80-infinity-through-bounded-m32-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31bz--masked-binary80-infinity-through-m32-fistfstp).

- **S2-31ca evidence:** bare `DF 10`/`DF 18` now convert Special-tagged
  canonical binary80 ±Infinity to Word integer indefinite `0x8000` under the
  admitted masked FCWs. After successful write, IE is sticky, C1 clears,
  incoming PE and C0/C2/C3 are preserved, and no other exception status is
  added. Tests cover signs/FCWs/opcodes/TOP/PE/C1, malformed/tag/class/control
  rejection, finite -32768 distinction, write/range atomicity, CLI/FNSTSW/FNCLEX,
  and JIT/native denial. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31ca-masked-canonical-infinity-fist-m16int),
  [plan](plans/STAGE_2.md#s2-31ca-masked-binary80-infinity-through-bounded-m16-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31ca--masked-binary80-infinity-through-m16-fistfstp).

- **S2-31cb evidence:** bare `DF 38` now converts Special-tagged canonical
  binary80 ±Infinity to Qword integer indefinite `0x8000000000000000` under
  admitted masked FCWs. After successful write, IE is sticky, C1 clears,
  incoming PE and C0/C2/C3 are preserved, and no other exception status is
  added. Coverage includes signs/FCW/TOP/PE/C1, finite -2^63 distinction,
  source/control/prefix rejection, Qword write/range atomicity, CLI/FNSTSW/FNCLEX,
  and JIT/native denial. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cb-masked-canonical-infinity-fistp-m64int),
  [plan](plans/STAGE_2.md#s2-31cb-masked-binary80-infinity-through-bounded-m64-fistp),
  and [report](reports/STAGE_2_REPORT.md#s2-31cb--masked-binary80-infinity-through-m64-fistp).


- **S2-31cc evidence:** bare `DB 10`/`DB 18` now admit Special-tagged
  canonical binary80 quiet NaNs (J=1, quiet bit=1) to m32 integer indefinite
  `0x80000000`. Successful writes add IE and clear C1 while preserving incoming
  PE/C0/C2/C3 and adding no other exception flags. Tests cover payloads/signs,
  FCW/opcode/TOP/PE/C1, rejection classes, write/range atomicity, CLI/FNSTSW/
  FNCLEX and JIT/native denial. Native runtime and physical acceptance remain
  deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cc-masked-canonical-qnan-fist-m32int),
  [plan](plans/STAGE_2.md#s2-31cc-masked-binary80-qnan-through-bounded-m32-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31cc--masked-binary80-qnan-through-m32-fistfstp).


- **S2-31cd evidence:** bare `DF 10`/`DF 18` now admit Special-tagged
  canonical binary80 quiet NaNs (J=1, quiet bit=1) to m16 integer indefinite
  `0x8000`. Successful writes add IE and clear C1 while preserving incoming
  PE/C0/C2/C3 and adding no other exception flags. Tests cover payloads/signs,
  FCW/opcode/TOP/PE/C1, rejection classes, Word write/range atomicity,
  CLI/FNSTSW/FNCLEX and JIT/native denial. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cd-masked-canonical-qnan-fist-m16int),
  [plan](plans/STAGE_2.md#s2-31cd-masked-binary80-qnan-through-bounded-m16-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31cd--masked-binary80-qnan-through-m16-fistfstp).


- **S2-31ce evidence:** bare `DF 38` now admits Special-tagged canonical
  binary80 quiet NaNs (J=1, quiet bit=1) to m64 integer indefinite
  `0x8000000000000000`. Successful writes add IE and clear C1 while preserving
  incoming PE/C0/C2/C3 and adding no other exception flags. Tests cover
  payloads/signs, FCW/TOP/PE/C1, rejection classes, Qword write/range atomicity,
  CLI/FNSTSW/FNCLEX and JIT/native denial. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31ce-masked-canonical-qnan-fistp-m64int),
  [plan](plans/STAGE_2.md#s2-31ce-masked-binary80-qnan-through-bounded-m64-fistp),
  and [report](reports/STAGE_2_REPORT.md#s2-31ce--masked-binary80-qnan-through-m64-fistp).
- **S2-31cf evidence:** bare `DB 10`/`DB 18` now admit canonical Special-tagged
  binary80 SNaNs for m32 FIST/FISTP, storing integer indefinite, adding IE,
  clearing C1 and preserving raw source bytes. The four-payload sign×FCW×opcode×TOP×PE/C1
  matrix has 2,048 cases; pseudo-SNaN/QNaN, tag, control, prefix, width, and
  write/range atomicity controls are included with CLI/FNSTSW and JIT denial.
  Native denial compiles for Linux x86_64 but is architecture-gated and did not
  run on the local arm64 host. Physical x86 and native runtime behavior remain
  unverified; S2-31 and
  ISSUE-019 remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cf-masked-canonical-snan-fist-m32int),
  [plan](plans/STAGE_2.md#s2-31cf-masked-binary80-snan-through-bounded-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cf--masked-binary80-snan-through-m32-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31cg evidence:** bare `DF 10`/`DF 18` now admit canonical Special-tagged
  binary80 SNaNs for m16 FIST/FISTP, storing Word integer indefinite, setting
  IE, clearing C1 and preserving raw source bytes. The four-payload
  sign×FCW×opcode×TOP×PE/C1 matrix has 2,048 cases; pseudo-NaN/tag/control/
  prefix and write/range atomicity controls, CLI/FNSTSW, JIT denial, and Linux
  x86_64 native-denial compile coverage are included. The former m64 `DF 38`
  denial was superseded by S2-31ch. Physical x86 and
  native runtime behavior remain unverified; S2-31 and ISSUE-019 remain open.
  See [contract](specs/CPU_CONTRACT.md#local-s2-31cg-masked-canonical-snan-fist-m16int),
  [plan](plans/STAGE_2.md#s2-31cg-masked-binary80-snan-through-bounded-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cg--masked-binary80-snan-through-m16-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ch evidence:** bare `DF 38` admits canonical Special-tagged binary80
  SNaNs for m64 `FISTP`, storing Qword integer indefinite, setting IE, clearing
  C1 and preserving raw source bytes. The 1,024-case payload/sign/FCW/TOP/
  PE/C1 matrix includes Qword write/range atomicity, malformed class/tag and
  control rejection, FNSTSW/FNCLEX, CLI fixture and JIT denial. Native denial
  is architecture-gated and compiled in the Linux x86_64 check; native runtime
  and physical behavior remain unverified. S2-31 and ISSUE-019 remain open.
  See [contract](specs/CPU_CONTRACT.md#local-s2-31ch-masked-canonical-snan-fistp-m64int),
  [plan](plans/STAGE_2.md#s2-31ch-masked-binary80-snan-through-bounded-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31ch--masked-binary80-snan-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ci evidence:** incoming sticky IE is admitted only at the five
  bounded integer-store gates (DB 10, DB 18, DF 10, DF 18, DF 38), alongside
  already accepted PE. DE/ZE/OE/UE/SF/ES/B remain rejected. The 17,920-case
  matrix covers accepted source classes/signs/forms/FCWs/TOP and historical
  C1; all-form IE+PE write/range atomicity and IE-plus-forbidden-status
  controls pass. Masked-invalid FIST→FISTP without FNCLEX, IE-only exact finite
  storage, FNSTSW/FNCLEX, two CLI fixtures and JIT/native denial are covered.
  Native runtime and physical x86 behavior remain unverified; S2-31 and
  ISSUE-019 remain open. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31ci-admit-sticky-ie-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31ci-admit-sticky-ie-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31ci--sticky-ie-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31cj evidence:** canonical finite binary80 Normal values that round
  outside signed i32 on bounded DB10/DB18 now store integer-indefinite and add
  IE after the atomic write. The exact quotient/remainder RC calculation is
  retained for exponents through 31, so values just outside the real range
  that round back into range remain finite; exponent 32 and above classify
  directly invalid. m16/m64, subnormal, malformed and unmasked cases remain
  outside this S2-31cj slice (m16 finite overflow is extended by S2-31ck and
  m64 finite overflow is added by S2-31cl).
  Native runtime and physical x86 checks remain unverified;
  S2-31 and ISSUE-019 remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cj-masked-finite-overflow-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cj-masked-finite-overflow-to-m32-fistfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31cj--masked-finite-overflow-to-m32-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ck evidence:** canonical finite binary80 Normal values rounded outside
  signed i16 through bounded DF10/DF18 now store Word integer-indefinite and
  add IE after the atomic write. Values that round back into range remain
  finite; exponent 16 and above classify invalid before wide shifts. The prior
  S2-31d/S2-31e overflow and exponent-16 fixtures now execute successfully,
  and S2-31ck adds
  CLI/FNSTSW examples for invalid overflow and roundback. m32 overflow,
  subnormal, malformed and unmasked behavior remain outside this slice; m64
  finite overflow is added by S2-31cl. Native
  runtime and physical x86 checks remain unverified; S2-31 and ISSUE-019 remain
  open. See [contract](specs/CPU_CONTRACT.md#local-s2-31ck-masked-finite-overflow-m16-fist),
  [plan](plans/STAGE_2.md#s2-31ck-masked-finite-overflow-to-m16-fistfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31ck--masked-finite-overflow-to-m16-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31cl evidence:** only bounded `DF 38` now converts canonical
  Valid-tagged binary80 Normal values whose RC-rounded integer is outside
  signed i64, storing Qword integer-indefinite and adding IE after the atomic
  write. Values outside the real bounds that round back into range remain
  finite, including the negative half-integer counterpart; exponent 64+ is
  classified without a wide shift. Subnormal, malformed and unmasked behavior
  remain unsupported. Focused overflow/fault/CLI coverage and final
  workspace/host/Linux check evidence are recorded in the linked report.
  Native execution and physical x86 behavior remain deferred; S2-31 and
  ISSUE-019 remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cl-masked-finite-overflow-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cl-masked-finite-overflow-to-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cl--masked-finite-overflow-to-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cm evidence:** bounded integer stores DB10/DB18/DF10/DF18/DF38 now
  admit sticky DE via the shared `0x80dc` status mask while preserving IE/DE/PE
  and rejecting ZE/OE/UE/SF/ES/B. DE alone adds no conversion flags. All five
  forms preserve full state and memory on denied-write/range faults with
  IE|DE|PE set; DE combined with each forbidden status rejects before access.
  FLD subnormal composition and FISTP/FNSTSW fixtures are documented in the
  linked report. The older S2-31d/e invalid-status fixtures now use ZE (0x4504)
  instead of DE (0x4502), which this slice admits. Native runtime and physical
  x86 behavior remain unverified;
  S2-31 and ISSUE-019 remain open. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cm-admit-sticky-de-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31cm-admit-sticky-de-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31cm--sticky-de-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cn evidence:** the same five bounded integer stores now admit and
  preserve sticky UE alongside IE/DE/PE via the shared `0x80cc` mask. UE alone
  does not alter the integer result or add IE/PE; current C1 is recomputed.
  The 71,680-case sticky matrix, 40-case FST m32 underflow composition, and
  five-form UE-bearing write/range atomicity plus UE|forbidden-status checks
  pass. New m32/m64 CLI fixtures verify UE survives FISTP and FNSTSW. JIT/native
  admission remains closed; native runtime and physical x86 behavior remain
  unverified, and S2-31/ISSUE-019 remain open. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cn-admit-sticky-ue-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31cn-admit-sticky-ue-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31cn--sticky-ue-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31co evidence:** bounded DB10/DB18/DF10/DF18/DF38 now admit historical
  OE through their shared `0x80c4` FSW gate alongside IE/DE/UE/PE; ZE/SF/ES/B
  remain rejected. Current conversion behavior, including integer-indefinite
  plus IE on invalid conversion, is independent of prior OE, and current C1 is
  recomputed. The 143,360-case matrix, actual FST m32 overflow composition,
  five-form atomicity/forbidden-status controls, OE-only conversion,
  FNSTSW/FNCLEX, CLI fixtures, and JIT/native denial are documented in the
  linked report. Native runtime and physical x86 acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31co-admit-sticky-oe-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31co-admit-sticky-oe-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31co--sticky-oe-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cp evidence:** bounded DB10/DB18/DF10/DF18/DF38 now admit historical
  ZE through the shared `0x80c0` FSW gate with IE/DE/OE/UE/PE; SF/ES/B remain
  rejected. ZE alone has no effect on the current conversion, generated flags,
  or C1. The 286,720-case matrix, ZE-only and current-PE/current-IE combinations,
  consecutive stores without FNCLEX, FNSTSW/FNCLEX, five-form all-six-flag fault
  atomicity, forbidden-status controls, literal fixtures, and JIT/native denial
  are captured in the report. No ZE-producing form or unmasked delivery was
  added. Binary80 subnormal and pseudo-denormal integer operands remain outside
  the bounded profile. Physical x86 acceptance remains deferred to the combined
  final S2-31 substage under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cp-admit-historical-sticky-ze-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31cp-admit-historical-sticky-ze-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31cp--historical-sticky-ze-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cq evidence:** only bare DB10/DB18 m32 integer stores now admit
  Special-tagged canonical binary80 subnormals. The complete 65,536-case
  sign/RC/opcode/TOP/sticky-flag/C1 matrix, FLD m80 composition, invalid-class
  controls, write/range atomicity, FISTP/FNSTSW CLI fixture, FNCLEX composition
  and JIT/native denial are tracked in the linked report. This does not resolve the broader
  S2-31/ISSUE-019 scope: native runtime and physical x86 acceptance remain
  deferred to the combined final substage. See [contract](specs/CPU_CONTRACT.md#local-s2-31cq-canonical-subnormal-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cq-canonical-subnormal-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cq--canonical-subnormal-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cr evidence:** only bare DF10/DF18 m16 integer stores now admit
  Special-tagged canonical binary80 subnormals. The linked report tracks the
  complete 65,536-case matrix, FLD m80 composition, rejection and atomicity
  controls, Word canaries, literal FISTP/FNSTSW fixture, and JIT/native
  denial. Native runtime and physical x86 acceptance remain deferred to the
  combined final S2-31 substage under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cr-canonical-subnormal-m16-fist),
  [plan](plans/STAGE_2.md#s2-31cr-canonical-subnormal-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cr--canonical-subnormal-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cs evidence:** only bare DF38 m64 FISTP now admits
  Special-tagged canonical binary80 subnormals. The 32,768-case
  fraction/sign/RC/TOP/sticky/C1 matrix, FLD m80 and FNSTSW/FNCLEX composition,
  malformed/tag/control rejection, Qword canaries, write/range atomicity,
  TOP wrap and raw source preservation, literal CLI fixture, and JIT/native
  denial are tracked in the report. Physical x86 acceptance and native runtime
  remain deferred to the combined final S2-31 substage under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cs-canonical-subnormal-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cs-canonical-subnormal-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cs--canonical-subnormal-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ct evidence:** only DB10/DB18 m32 FIST/FISTP admit Special-tagged
  J=0 pseudo-infinity (zero significand, all-one exponent, either sign),
  storing integer-indefinite and adding IE. The 16,384-case matrix checks all
  FCWs, forms, TOP values, six sticky flags and old C1; additional coverage
  checks pseudo-NaNs/tags/widths, raw-source preservation, status observation,
  FNCLEX, canaries, write/range atomicity, CLI and JIT/native denial. Physical
  x86 acceptance and native runtime remain deferred to the combined final
  S2-31 substage under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31ct-pseudo-infinity-m32-fist),
  [plan](plans/STAGE_2.md#s2-31ct-pseudo-infinity-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31ct--special-tagged-pseudo-infinity-through-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cu evidence:** only bare DF10/DF18 m16 FIST/FISTP now admit a
  Special-tagged binary80 pseudo-infinity with raw sign/exponent `0x7fff` or
  `0xffff` and zero significand. They store Word integer-indefinite `0x8000`,
  add IE, clear C1, add no PE/other new flags, preserve sticky/C0/C2/C3, and
  commit after the atomic Word write. The 16,384-case matrix checks FCW/form/
  TOP/sticky/C1, source bytes, stack and status; rejection, Word canary,
  denied-write/range atomicity, FNSTSW/FNCLEX, literal CLI, JIT denial and
  Linux x86_64 cross-target native-denial compile coverage are included.
  Linking the Linux test binary is unavailable with the macOS host linker;
  native runtime and physical x86 acceptance remain deferred under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cu-pseudo-infinity-m16-fist),
  [plan](plans/STAGE_2.md#s2-31cu-pseudo-infinity-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cu--special-tagged-pseudo-infinity-through-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cv evidence:** only bare DF38 m64 FISTP now admits Special-tagged
  pseudo-infinity with zero significand and raw sign/exponent `0x7fff` or
  `0xffff`. The 8,192-case FCW/TOP/sticky/C1 matrix, Qword indefinite result,
  IE-without-new-PE status, raw source retention, TOP wrap, atomic write/range
  rejection, malformed-class/tag/control checks, seeded FNSTSW/FNCLEX CLI
  fixture, and JIT/native denial are recorded in the linked report. FLD
  composition is excluded. Native runtime and physical x86 acceptance remain
  deferred to the combined final S2-31 substage under ISSUE-019; Mac mini
  validation remains pending. See [contract](specs/CPU_CONTRACT.md#local-s2-31cv-pseudo-infinity-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cv-pseudo-infinity-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cv--special-tagged-pseudo-infinity-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cw evidence:** DB10/DB18 m32 FIST/FISTP admit only Special-tagged
  J=0 binary80 pseudo-NaNs (all-one exponent, nonzero fraction), including
  Q-bit-clear and Q-bit-set payloads of either sign. A 65,536-case
  independently expected matrix checks integer-indefinite, IE without PE,
  C1 clearing, sticky/condition status preservation, source bytes, Dword
  canaries, single-write/no-read and TOP/tag rules. Wrong tags, pseudo-infinity,
  canonical J=1 QNaN/SNaN, non-all-one exponent, m16/m64, FCW/FSW controls and
  write/range atomicity are covered. The direct CLI fixture observes IE with
  FNSTSW; an ISA FNSTSW/FNCLEX sequence clears it. JIT/native denial is also
  covered; FLD, unmasked behavior and physical x86 acceptance remain outside
  this bounded slice. The latter stays deferred to
  the combined final S2-31 substage under ISSUE-019. See [S2-31cw CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31cw-special-pseudo-nan-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cw-special-pseudo-nan-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cw--special-tagged-pseudo-nans-through-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31cx evidence:** only bare DF10/DF18 m16 FIST/FISTP admit
  Special-tagged J=0 binary80 pseudo-NaNs with nonzero fraction, including
  both bit-62 variants and both signs. The 65,536-case independent matrix
  checks Word integer-indefinite, IE without PE, C1 clearing, sticky/condition
  preservation, source bytes, canaries, one write/no read and TOP/tag rules.
  Wrong tags/classes, other exponents, m64, forbidden controls/status and
  denied-write/canonical-range atomicity are covered; FNSTSW/FNCLEX, literal
  CLI, and JIT/native denial checks are included. The m32 behavior remains
  unchanged. FLD and unmasked delivery remain outside this bounded slice.
  Physical x86 acceptance and Mac mini validation remain deferred to the
  combined final S2-31 substage under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cx-special-pseudo-nan-m16-fist),
  [plan](plans/STAGE_2.md#s2-31cx-special-pseudo-nan-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cx--special-tagged-pseudo-nans-through-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cy evidence:** only bare DF38 m64 FISTP admits a Special-tagged J=0
  pseudo-NaN with exponent `0x7fff` and nonzero fraction, including either
  bit-62 variant and both signs. The 32,768-case independent matrix checks
  Qword integer-indefinite, IE/no new PE, C1 and sticky/condition preservation,
  raw ten-byte payload, Qword canaries, one write/no read and TOP wrap. Neighbor
  classes/tags, controls, write/range atomicity, FNSTSW/FNCLEX, a seeded CLI
  fixture, and JIT/native denial are covered. m16/m32 regression behavior is
  unchanged; FLD/unmasked behavior is excluded. Mac mini and physical x86
  acceptance remain deferred to the combined final S2-31 substage under this
  issue. See [contract](specs/CPU_CONTRACT.md#local-s2-31cy-special-pseudo-nan-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cy-special-pseudo-nan-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cy--special-tagged-pseudo-nan-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cz evidence:** only bare DB10/DB18 m32 FIST/FISTP admit a
  Special-tagged unnormal with biased exponent 1..=`0x7ffe`, J=0 and arbitrary
  fraction. The independent 196,608-case exponent/fraction/sign/FCW/form/TOP/
  sticky/C1 matrix checks Dword indefinite, IE without PE/DE, status and raw
  payload preservation, canaries, one write/no read and TOP/tag behavior.
  Boundary/source/control rejects, write/range atomicity, FNSTSW/FNCLEX, a
  seeded CLI fixture, and JIT/native denial are covered. Physical x86 and Mac
  mini acceptance remain deferred to the combined S2-31 substage; ISSUE-019
  remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cz-special-unnormals-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cz-special-unnormals-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cz--special-tagged-unnormals-through-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31da evidence:** bare DF10/DF18 m16 FIST/FISTP now admit only
  Special-tagged finite-exponent binary80 unnormals (J=0); the 196,608-case
  literal matrix checks Word indefinite, IE without PE/DE, flags/status, raw
  payload, canaries and stack behavior. Boundary/tag/width/control rejects,
  write/range atomicity, FNSTSW/FNCLEX, CLI fixture and JIT/native denial are
  covered. Mac mini and physical x86 acceptance remain deferred; ISSUE-019
  stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-31da-special-unnormals-m16-fist),
  [plan](plans/STAGE_2.md#s2-31da-special-unnormals-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31da--special-tagged-unnormals-through-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31db evidence:** bare DF38 admits only Special-tagged finite-exponent
  binary80 unnormals (J=0) for m64 FISTP; the 98,304-case matrix checks Qword
  indefinite, IE without PE/DE, status/payload preservation, canaries, one
  write/no read, and pop behavior. Boundary/tag/control rejects, status
  visibility, CLI fixture, and JIT/native denial are covered. Mac mini and
  physical x86 acceptance remain deferred; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-31db-special-unnormals-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31db-special-unnormals-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31db--special-tagged-unnormals-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31do evidence:** bare Long64 `D9 FC` admits canonical Special-tagged
  nonzero binary80 subnormals and adds current DE+PE with signed-zero/±1
  rounding and Zero/Valid result tags. The independent state matrix, atomic
  neighbors, repeated FRNDINT, FNSTSW/FNCLEX and seeded
  `project_fixture` composition are local evidence only. JIT/native, physical
  x86 and Mac mini acceptance remain deferred to the combined final S2-31
  substage; ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31do-canonical-binary80-subnormal-frndint-d9-fc),
  [plan](plans/STAGE_2.md#s2-31do-canonical-binary80-subnormal-through-bounded-frndint),
  [report](reports/STAGE_2_REPORT.md#s2-31do--canonical-binary80-subnormal-through-bounded-frndint),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31do-frndint-canonical-subnormal-fixture).
- **S2-31dp evidence:** bare Long64 `D9 FC` additionally admits Special-tagged
  binary80 pseudo-denormals (E=0/J=1/any fraction) through the bounded FRNDINT
  path. Local equivalence matrices, neighbor rejects, FLD→FRNDINT→FNSTSW
  project fixture, re-entry and status-clearing coverage are recorded in the
  linked report. Native/JIT and physical x86/Mac mini acceptance remain
  deferred; ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dp-special-tagged-binary80-pseudo-denormal-frndint-d9-fc),
  [plan](plans/STAGE_2.md#s2-31dp-special-tagged-binary80-pseudo-denormal-through-bounded-frndint),
  [report](reports/STAGE_2_REPORT.md#s2-31dp--special-tagged-binary80-pseudo-denormal-through-bounded-frndint),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31dp-frndint-pseudo-denormal-fixture).
- **S2-31dt evidence:** exact bare D900/DD00 now admit and preserve historical UE through their local `0x80cc` gates; ZE/OE/SF/ES/B remain pre-read rejects, and unrelated FILD/m80/store/FRNDINT/native/JIT predicates are unchanged. The interpreter matrix crosses both widths, eight source classes, four FCWs, all TOPs, prior C1 and all IE/DE/UE/PE combinations with complete state and memory checks. UE-only normal-load `FNSTSW` project fixtures, current SNaN IE/quieting and subnormal DE, same/cross-width underflow-store→load re-entry, repeated loads and FNCLEX are covered. Bad controls, occupied destinations, forbidden UE combinations, backend/range/read/boundary atomicity remain checked. Physical x86 and native runtime behavior were not performed; ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dt-historical-sticky-ue-for-bounded-m32m64-fld), [plan](plans/STAGE_2.md#s2-31dt-admit-historical-sticky-ue-through-exact-binary32binary64-fld), [report](reports/STAGE_2_REPORT.md#s2-31dt--historical-sticky-ue-for-bounded-m32m64-fld), and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-30as bounded FSTP ST(i) signed-zero evidence:** exact Zero-tagged ±0 is copied with its tag to `(old_TOP+i)&7`, the old physical TOP is then marked Empty, and TOP advances once. The `i=0` payload remains with final Empty tag; `i=1` becomes new ST(0). Normal FSTP behavior remains covered. Local matrices and both-width compositions plus exact fixture `d900ddd9dfe0` do not establish native/JIT or physical behavior; ISSUE-019 remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-30as-signed-zero-through-fstp-sti-dd-d8i), [plan](plans/STAGE_2.md#s2-30as-admit-canonical-signed-zero-to-bare-fstp-sti-dd-d8i), [report](reports/STAGE_2_REPORT.md#s2-30as--canonical-signed-zero-through-fstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30as-signed-zero-fstp-sti).
- **S2-30bc historical IE evidence:** the shared `FCHS/FABS` source gate admits IE only by changing its FSW mask to `0x80fe`; other exception/summary bits remain rejected, including IE+SF/DE. The 6,144-case state matrix, same-width m32/m64 SNaN compositions, schema-v2 fixtures, and status observation/clearing checks are local evidence only. At that substage, DE and IE+DE were still rejected; S2-30bd supersedes that status restriction. Native/JIT and physical x86 behavior remain unverified; ISSUE-019 remains open for combined acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bc-historical-ie-fchs-fabs), [plan](plans/STAGE_2.md#s2-30bc-admit-historical-sticky-ie-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bc--historical-sticky-ie-through-bounded-fchs-fabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bc-historical-ie-fchs-fabs).
- **S2-30bd historical IE/DE evidence:** the shared `FCHS/FABS` gate now admits sticky DE alongside IE via FSW mask `0x80fc`; all other forbidden status bits remain closed. The 12,288-state matrix, binary32/64 subnormal load/sign/store compositions, and project fixtures are local evidence only. Native/JIT and physical behavior remain unverified; ISSUE-019 stays open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bd-historical-de-fchs-fabs), [plan](plans/STAGE_2.md#s2-30bd-admit-historical-sticky-de-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bd--historical-sticky-de-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bd-historical-de-fchs-fabs).

- **S2-30be historical IE/DE/PE evidence:** the bounded `FCHS/FABS` source gate now uses FSW mask `0x80dc`, admitting exactly IE/DE/PE combinations and continuing to reject ZE/OE/UE/SF/ES/B combinations. The 24,576-state matrix, FRNDINT PE producer checks, signed-zero subnormal→FRNDINT→FCHS→FABS checks of complete raw/tag/FSW state, six schema-v2 CLI fixtures, and JIT denial are local evidence. Native/runtime and physical x86/Mac mini acceptance remain deferred; ISSUE-019 stays open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30be-historical-pe-fchs-fabs), [plan](plans/STAGE_2.md#s2-30be-admit-historical-sticky-pe-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30be--historical-sticky-pe-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30be-historical-pe-fchs-fabs).

- **S2-30bf historical IE/DE/UE/PE evidence:** the bounded sign-operation source gate now uses mask `0x80cc`, admitting all 16 sticky IE/DE/UE/PE combinations and rejecting ZE/OE/SF/ES/B across each. Local 49,152-case matrix, masked-underflow FST producers and CLI status fixtures do not establish native/JIT execution or physical x86 behavior. ISSUE-019 remains open; next step is combined final-substage acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bf-historical-ue-fchs-fabs), [plan](plans/STAGE_2.md#s2-30bf-admit-historical-sticky-ue-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bf--historical-sticky-ue-through-bounded-fchs-fabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bf-historical-ue-fchs-fabs).
- **S2-30bg historical IE/DE/OE/UE/PE evidence:** the bounded sign-operation source gate now uses mask `0x80c4`, admitting all 32 sticky histories while preserving ZE/SF/ES/B atomic rejects. The 98,304-state full matrix, 3,072 forbidden-status cases, real m32/m64 FST overflow-midpoint producers and neighbors, seeded OE CLI case, ZE rejection fixtures, and JIT denial are local evidence only. No Mac mini, SSH, native runtime or physical x86 checks were performed. ISSUE-019 remains open for combined host/physical acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bg-historical-oe-through-bounded-fchsfabs), [plan](plans/STAGE_2.md#s2-30bg-admit-historical-sticky-oe-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bg--historical-sticky-oe-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bg-historical-oe-through-bounded-fchsfabs).

- **S2-30bh historical IE/DE/ZE/OE/UE/PE evidence:** the bounded `FCHS/FABS`
  gate uses mask `0x80c0`, admitting all 64 sticky histories while SF/ES/B
  remain atomic rejects. The 196,608 full-state matrix, 4,608 forbidden-status
  product, seeded ZE/all-flags CLI fixtures, FRNDINT compositions and JIT denial
  are local evidence only. ZE is explicitly seeded; no divide-by-zero producer
  is claimed. No Mac mini, SSH, native runtime or physical x86 checks were run.
  ISSUE-019 remains open for combined final acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bh-historical-ze-through-bounded-fchsfabs),
  [plan](plans/STAGE_2.md#s2-30bh-admit-historical-sticky-ze-to-bounded-fchsfabs),
  [report](reports/STAGE_2_REPORT.md#s2-30bh--historical-sticky-ze-through-bounded-fchsfabs),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bh-historical-ze-through-bounded-fchsfabs).
- **S2-30bi historical SF|IE evidence:** the bounded FCHS/FABS gate accepts
  SF only when IE is also set; standalone SF and SF|DE without IE still reject.
  The 294,912-state full-state matrix, 768 no-IE SF rejects, 6,144 ES/B cross
  product, seeded basic and FRNDINT-composition CLI fixtures, and execution
  boundaries are local evidence only. IE|SF is seeded in the initial project
  snapshot; there is no guest SF producer in scope, and FRNDINT only composes
  with the seeded history. Empty, ES, B, #MF/WAIT delivery and native/physical
  behavior remain outside this slice. Further ES/B relaxation requires a
  separate pending-exception contract plus final physical x86 acceptance.
  ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bi-historical-sf-ie-fchs-fabs),
  [plan](plans/STAGE_2.md#s2-30bi-admit-historical-sf-only-with-ie-to-bounded-fchsfabs),
  [report](reports/STAGE_2_REPORT.md#s2-30bi--historical-sf-only-with-ie-through-bounded-fchsfabs),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bi-historical-sfie-through-bounded-fchsfabs).
- **S2-30bm bounded TOP adjustment evidence:** bare `D9 F7`/`D9 F6` now adjust only the TOP field, clear C1 and preserve all eight raw cells plus the full tag word without inspecting them. The 1,536-case opaque-state matrix, wrap/repeat/inverse checks, all nine status-gate rejects, schema allowlist, and seeded status-observation fixtures are local evidence. Preserving undefined C0/C2/C3 is a local deterministic policy. ES/B, #MF/#NM/WAIT/pending-exception/IE/SF behavior, JIT/native execution, and physical x86 acceptance remain outside this slice; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30bm-bounded-register-only-fincstp--fdecstp), [plan](plans/STAGE_2.md#s2-30bm-bounded-register-only-fincstp--fdecstp), [report](reports/STAGE_2_REPORT.md#s2-30bm--bounded-register-only-fincstp-and-fdecstp), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bm-bounded-fincstpfdecstp).
- **S2-30bt bounded FXAM evidence:** exact bare `D9 E5` classifies only Valid-tagged canonical Normal and Zero-tagged exact signed zero under the strict FCW/status gate. The 5,120 success states cover four canonical Normal representatives (minimum normal, 1, 1+ULP, maximum finite) and exact Zero; the 10,752 class/tag rejects cover each nonadmitted class/tag pair under both signs, TOP values and prior CC patterns. Seven exact schema streams, literal producer/repeat/sign fixtures and five atomic rejects are local interpreter evidence; JIT/native remain denied and physical behavior remains unverified. FTST's gate is unchanged. ISSUE-019 remains open for broader x87 semantics and final host/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30bt-bounded-bare-fxam-d9-e5), [plan](plans/STAGE_2.md#s2-30bt-bounded-bare-fxam-normal-and-signed-zero-classification), [report](reports/STAGE_2_REPORT.md#s2-30bt--bounded-bare-fxam), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bt-bounded-fxam).
- **S2-30bz indexed FCMOVE evidence:** exact bare `DA C8..CF` uses the low ModR/M bits as the physical source index. For nonzero indices it validates old TOP and `(TOP+i)&7` before integer RFLAGS.ZF; self-index validates only TOP. The 363,520 success and 161,280 invalid class/tag snapshots, unchanged FCW/FSW gates, exact 19-stream schema, indexed fixtures and complete runner x87 snapshots are local-only evidence; FSW.C3 is not the condition. JIT/native execution, exception delivery and physical acceptance remain unresolved under ISSUE-019. The prior ST(1)-only scope is historical and superseded. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bz-bounded-indexed-bare-fcmove-st0-sti-da-c8cf), [plan](plans/STAGE_2.md#s2-30bz-bare-fcmove-st0-sti-da-c8cf), [report](reports/STAGE_2_REPORT.md#s2-30bz--bounded-indexed-fcmove-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bz-bounded-indexed-fcmove-st0-sti).

- **S2-30cf indexed FCMOVNBE acceptance remains local:** bare `DB D0..D7` now uses the strict shared x87 environment and participant gates; local matrices, exact schema, producer/repeat fixtures, complete runner snapshots and JIT denial are recorded in the S2-30cf report. Native runtime, x87 exception delivery and physical x86/Mac mini acceptance have not been performed. Status: open under ISSUE-019; next step is combined acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30cf-bounded-indexed-fcmovnbe-st0-sti-db-d0d7), [plan](plans/STAGE_2.md#s2-30cf-indexed-bare-fcmovnbe-st0-sti-db-d0d7), [report](reports/STAGE_2_REPORT.md#s2-30cf--indexed-bare-fcmovnbe-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cf-indexed-bare-fcmovnbe-st0-sti-db-d0d7).
- **S2-30ce historical FCMOVNBE extent:** exact unprefixed Long64 `DB D1` had a local ST(1)-only interpreter slice with strict x87 environment/operand preflight and raw copy only when integer CF and ZF were both clear. That bounded scope is superseded by indexed S2-30cf; its 51,200 success and 86,016 invalid participant matrices and original six-stream/producer coverage remain historical evidence. Exception delivery, native/JIT execution and physical x86 acceptance remain unresolved under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-30ce-bounded-bare-fcmovnbe-st0-st1-db-d1), [plan](plans/STAGE_2.md#s2-30ce-exact-bare-fcmovnbe-st0-st1-db-d1), [report](reports/STAGE_2_REPORT.md#s2-30ce--bare-fcmovnbe-st0-st1), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ce-bare-fcmovnbe-st0-st1-db-d1).
- **S2-30cg FCMOVU historical extent:** its original exact `DA D9` ST(1)-only local slice covered 512 success and 768 invalid class/tag snapshots. Those counts describe only the earlier one-index matrix; S2-30ch supersedes it for indexed execution. Its two TEST producer fixtures remain compatibility evidence. No native/JIT or physical x86 behavior is established.
- **S2-30ch indexed FCMOVU evidence:** exact unprefixed Long64 `DA D8..DF` carries the source index, preflights TOP and `(TOP+i)&7` before PF for `i>0`, and checks TOP alone for self-index. PF=1 raw-copies the exact ten bytes/tag; PF=0 retains the destination. The bounded profile remains FCW `0x037f`, `FSW & 0x82ff == 0`, Valid canonical Normal or Zero exact signed zero; C1 and Special/NaN semantics remain closed. Local coverage includes 363,520 successes, 161,280 invalid class/tag snapshots, per-index environment/boundary checks, 18 exact schema streams, nine new bare success fixtures, repeat, two TEST producer fixtures, rejection fixtures and JIT denial. Native/JIT execution, exception delivery and physical acceptance remain unverified; ISSUE-019 stays open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30ch-bounded-indexed-fcmovu-st0-sti-da-d8df), [plan](plans/STAGE_2.md#s2-30ch-indexed-bare-fcmovu-st0-sti-da-d8df), [report](reports/STAGE_2_REPORT.md#s2-30ch--indexed-bare-fcmovu-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ch-indexed-bare-fcmovu-st0-sti-da-d8df).

**S2-30ck local FSQRT slice:** exact unprefixed Long64 `D9 FA` now executes in
the interpreter only for Valid-tagged canonical positive nonzero Normal
binary80 at physical TOP, under the twelve all-masked PC24/53/64 FCW values and
`FSW & 0x80df == 0`. The full 1,536-state success matrix, literal direct-rounding
vectors and atomic class/environment/boundary rejects are local checks. This
assumes x87 available, CR0.EM=CR0.TS=0 and no pending exception. The existing
selector-bearing x87 gate remains unchanged; FDP is preserved locally, and
CPUID.07H.00H:EBX[6] concerns FDP behavior only. General x87 exceptions,
pending delivery, native hardware and physical acceptance remain unresolved.
See [S2-30ck contract](specs/CPU_CONTRACT.md#local-s2-30ck-bounded-register-only-fsqrt-st0-d9-fa),
[plan](plans/STAGE_2.md#s2-30ck-bare-register-only-fsqrt-st0-d9-fa), and
[report](reports/STAGE_2_REPORT.md#s2-30ck--bounded-bare-fsqrt-st0).

**S2-30cl local FPREM slice:** exact unprefixed Long64 `D9 F8` is admitted only
for positive canonical Normal operands, all-masked FCW, clean required FSW
fields, exponent gap below 64 and a divisor exponent cutoff that proves results
normal or zero. The local full-reduction path updates the exact quotient CCs;
partial reduction, underflow, general exception delivery, JIT/native execution
and physical acceptance remain outside scope. The 3,072-state matrix, literal
quotient/remainder vectors, atomic denials and JIT denial pass. Native denial
was added but could not be executed under ISSUE-029. See [S2-30cl contract](specs/CPU_CONTRACT.md#local-s2-30cl-bounded-register-only-fprem-st0-st1-d9-f8),
[plan](plans/STAGE_2.md#s2-30cl-bare-register-only-fprem-st0-st1-d9-f8), and
[report](reports/STAGE_2_REPORT.md#s2-30cl--bounded-bare-fprem-st0-st1).

**S2-30cm local FPREM1 slice:** exact unprefixed Long64 `D9 F5` executes only
for Valid canonical positive Normal operands, all-masked FCW, clean required
FSW fields, complete-reduction gap `D<64`, and the proven local divisor cutoff
`e1>=-16318`. The signed nearest-even remainder helper is used for all admitted
gaps including `D=-1`; partial reduction and subnormal results are excluded.
Full-state matrices, literal quotient/remainder cases, atomic denials and JIT
denial pass. Linux-only native denial coverage is present but was not run on
this macOS host; ISSUE-029 tracks that unverified backend route. Physical x86
acceptance and general x87 exception delivery remain open. See [S2-30cm
contract](specs/CPU_CONTRACT.md#local-s2-30cm-bounded-register-only-fprem1-st0-st1-d9-f5),
[plan](plans/STAGE_2.md#s2-30cm-bare-register-only-fprem1-st0-st1-d9-f5), and
[report](reports/STAGE_2_REPORT.md#s2-30cm--bounded-bare-fprem1-st0-st1).

**S2-30cn local FDIV slice:** exact bare `D8 F0..F7` executes for Valid
canonical nonzero Normal operands, all-masked PC24/53/64 FCW, and clean required
FSW fields. It uses the direct finite division precision helper; only finite
Normal results are admitted. PE accumulates mathematical inexactness and C1
uses the helper's pre-carry increment diagnostic; C0/C2/C3 are preserved by
local policy. DC/DE directions and pop forms retain their prior exact-only
scope; bounded reverse `FDIVR ST(0), ST(i)` is the separate S2-30cs slice.
Literal tests, full-state denials, `od-isa`, `od-testkit`,
runner, JIT-denial, workspace and clean Sol/high review results are recorded in
[S2-30cn report](reports/S2_30CN_X87_FDIV_PRECISION.md). Native/JIT execution,
guest exception delivery and physical x86 acceptance remain outside scope;
ISSUE-019 remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-30cn-bounded-fdiv-st0-sti-d8-f0-f7),
[plan](plans/STAGE_2.md#s2-30cn-bare-fdiv-st0-sti-with-bounded-fcw-precision), and
[shared report](reports/STAGE_2_REPORT.md#s2-30cn--bounded-fdiv-st0-sti).

**S2-30cs local FDIVR slice:** exact bare Long64 `D8 F8..FF` directly rounds
`ST(i) / ST(0)` through the existing finite division precision helper, with
all-masked PC24/53/64 FCWs and the same strict x87 state, operand and normal
result boundary as S2-30cn. It writes old physical TOP without popping, ORs PE,
replaces C1 from magnitude `incremented`, preserves C0/C2/C3 and other state,
and retires once. The p64 1/3 rejection is superseded by a positive rounded
case; actual zero/special/subnormal/malformed, unsupported environment and
out-of-domain cases remain atomic refusals. Full TOP/index/sign/FCW, midpoint,
carry, order, sticky status, fetch/STOP/budget and canary coverage is recorded
in [S2-30cs report](reports/S2_30CS_X87_FDIVR_PRECISION.md). JIT/native,
guest exception delivery and physical x86 acceptance remain outside scope;
ISSUE-019 remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-30cs-bounded-fdivr-st0-sti-d8-f8ff),
[plan](plans/STAGE_2.md#s2-30cs-bare-fdivr-st0-sti-with-bounded-fcw-precision),
and [shared report](reports/STAGE_2_REPORT.md#s2-30cs--bounded-fdivr-st0-sti).

**S2-30co local FMUL slice:** exact bare `D8 C8..CF` now uses target-precision
binary80 multiplication only for Valid canonical nonzero Normal operands and
the twelve all-masked FCW settings. The self-alias squares and can be inexact;
PE accumulates `inexact` and C1 uses pre-carry magnitude `incremented`. Other
FMUL directions and FMULP remain exact-only. Focused/full interpreter tests,
runner regressions, workspace checks, and independent Sol/high review pass.
No native/JIT or physical x86 support is claimed. See [contract](specs/CPU_CONTRACT.md#local-s2-30co-bounded-fmul-st0-sti-d8-c8-cf),
[plan](plans/STAGE_2.md#s2-30co-bare-fmul-st0-sti-with-bounded-fcw-precision),
and [report](reports/STAGE_2_REPORT.md#s2-30co--bounded-fmul-st0-sti).

**S2-30cp local FADD slice:** bare `D8 C0..C7` now uses direct target
precision for Valid canonical nonzero Normal operands under the twelve
all-masked FCWs, with a normalized exponent-gap limit of 64 enforced before
the helper call. PE accumulates `inexact`, C1 uses pre-carry magnitude
`incremented`, and the alias `i=0` doubles ST(0). Reverse, pop and subtraction
forms remain exact-only. Focused/full interpreter tests, runner regressions,
workspace checks, formatting and independent Sol/high review pass. JIT/native
and physical x86 acceptance are not claimed. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30cp-bounded-fadd-st0-sti-d8-c0-c7),
[plan](plans/STAGE_2.md#s2-30cp-bare-fadd-st0-sti-with-bounded-fcw-precision),
and [standalone report](reports/S2_30CP_X87_FADD_PRECISION.md). S2-30 and
ISSUE-019 remain open.

**S2-30cq local FSUB slice:** bare `D8 E0..E7` now uses the same bounded
target precision, FCWs, operand gates, gap≤64 limit and PE/C1 commit policy.
It flips only a validated local copy of the RHS sign bit and reuses the
existing finite-addition precision helper; `i=0` cancellation and all other
zero/subnormal/non-Normal results reject atomically. Focused direct tests cover
all TOP/index pairs, signs and FCWs, p53 midpoint neighbors and directed
rounding, gaps 1/63/64 and 65+, sticky PE/C1, malformed/range refusals,
fetch/STOP/budget and no-data-access behavior. Focused, full od-isa,
testkit/runner, workspace, formatting and diff checks passed. Graphify AST
refresh passed. Layout validation reproduces the existing `.git` worktree
pointer and missing historical oracle manifest findings in ISSUE-023;
independent Sol/high review is clean after correcting the refusal-test baseline
and duplicate admission check. JIT/native and physical x86 behavior are not
claimed. See [CPU
contract](specs/CPU_CONTRACT.md#local-s2-30cq-bounded-fsub-st0-sti-d8-e0-e7),
[plan](plans/STAGE_2.md#s2-30cq-bare-fsub-st0-sti-with-bounded-fcw-precision),
and [standalone report](reports/S2_30CQ_X87_FSUB_PRECISION.md). ISSUE-019
remains open.

**S2-30cr local FSUBR slice:** bare `D8 E8..EF` now uses the bounded target
precision, twelve all-masked FCWs, canonical Normal operand gates, normalized
gap≤64 limit and PE/C1 commit policy. It computes `ST(i) - ST(0)` by flipping
only a validated local copy of ST(0) and reusing the existing finite-addition
precision helper. `i=0` cancellation and zero/subnormal/overflow/non-Normal
results reject atomically. Direct checks cover all TOP/index pairs, sign
combinations and FCWs, p53 midpoint neighbors, directed rounding for both
result signs, gaps 1/63/64 and 65+, selectors/tags/status/malformed/range
denials, source/canaries, fetch/STOP/budget and no-data-access behavior. Focused
and full od-isa, testkit/runner, workspace, formatting, layout, diff and
Graphify checks are recorded in [S2-30cr report](reports/S2_30CR_X87_FSUBR_PRECISION.md).
Independent Sol/high review completed clean. JIT/native and physical x86
acceptance remain open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30cr-bounded-fsubr-st0-sti-d8-e8-ef),
[plan](plans/STAGE_2.md#s2-30cr-bare-fsubr-st0-sti-with-bounded-fcw-precision),
and [shared report](reports/STAGE_2_REPORT.md#s2-30cr--bounded-fsubr-st0-sti).

### ISSUE-020 — Не подтверждён победитель dual-fault для CMPS RSI/RDI

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-isa`, локальные Long64 CMPS B/W/D/Q S2-18i/S2-18n, REPE CMPSB/W/D/Q S2-19f/S2-19h, REPE CMPSB address-size-32 S2-19n, REPE CMPSW/D/Q address-size-32 S2-19p, REPNE CMPSB address-size-32 S2-19r и REPNE CMPSW/D/Q address-size-32 S2-19t.
- **Статус:** открыта; это не подтверждённый дефект.
- **Суть и последствия:** bounded CMPS profiles читают width-sized flat DS:[RSI]/DS:[ESI] до flat ES:[RDI]/ES:[EDI]. REP CMPS B/W/D/Q коммитит compare/index/count только после обоих чтений; если второе чтение отказывает, первый read остаётся в trace. Address-size-32 S2-18n separately records four dual-unmapped fixtures (`s2_18n_dual_unmapped_b.case.json` and `_w`, `_d`, `_q`) and expects the local ESI-first error; S2-19n, S2-19p, REPNE S2-19r and S2-19t preserve source-first behavior for dual-invalid addresses. At two simultaneously invalid addresses this policy does not establish hardware exception priority. The implementation makes no MMIO/backend side-effect rollback claim.
- **Основание:** S2-18i, S2-18n, S2-19f, S2-19h, S2-19n, S2-19p, S2-19r and S2-19t specify RSI/ESI-first local policy. Unit tests/fault matrices check first/second failures, replay and the local dual-invalid result, but do not compare it with hardware. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-19r-bounded-repne-cmpsb-address-size-32-contract), [S2-18n contract](specs/CPU_CONTRACT.md#local-s2-18n-non-rep-address-size-32-cmps-contract), [S2-19h plan](plans/STAGE_2.md#s2-19h--bounded-f3-a7-repe-cmpswdq), [S2-19n plan](plans/STAGE_2.md#s2-19n--bounded-repe-cmpsb-с-address-size-32), [S2-19p plan](plans/STAGE_2.md#s2-19p--bounded-repe-cmpswdq-с-address-size-32), [S2-19r plan](plans/STAGE_2.md#s2-19r--bounded-repne-cmpsb-с-address-size-32), and [S2-19t plan](plans/STAGE_2.md#s2-19t--bounded-repne-cmpswdq-addr32).
- **Следующий шаг:** если потребуется аппаратный dual-fault claim, сначала найти нормативное evidence либо сформировать изолированный dual-invalid case, затем проверить его на physical x86 в общем финальном acceptance substage. До такого результата сохранять локальную политику и не выдавать её за hardware priority.

### ISSUE-021 — Не подтверждён приоритет dual-invalid для MOVS RSI/RDI

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-isa`, локальный Long64 REP MOVSB/W/D/Q профиль S2-19d/S2-19l и non-REP address-size-32 MOVS S2-18o.
- **Статус:** открыта; это не подтверждённый дефект.
- **Суть и последствия:** bounded interpreter performs source read `DS:[RSI/ESI]` before destination write `ES:[RDI/EDI]`, including the S2-19l address-size-32 REP form. This local order means a source failure prevents destination access; it does not establish hardware exception priority when both are invalid. Destination failure follows a successful source read and precedes commit of destination bytes and index/count state for that iteration under the bounded all-or-error memory contract. No MMIO rollback is claimed.
- **Основание:** S2-19d and S2-19l `rep_movs_faults_preserve_only_fully_transferred_iterations` matrices check source-first traces and retry behavior. S2-18o adds `movs_address32_dual_invalid_accesses_observe_local_source_first_policy` and four `s2_18o_dual_unmapped_{b,w,d,q}.case.json` fixtures. These local tests are not a physical x86 comparison.
- **Следующий шаг:** при необходимости аппаратного dual-fault claim найти нормативное основание либо провести изолированное dual-invalid сравнение на physical x86 в общем финальном acceptance substage. До результата оставлять порядок source-first только local policy. Реализация S2-19l описана в [плане](plans/STAGE_2.md#s2-19l-bounded-rep-movs-с-address-size-32) и [контракте](specs/CPU_CONTRACT.md#local-s2-19l-bounded-rep-movs-address-size-32).

### ISSUE-023 — Layout checker reports worktree metadata and stale oracle links

- **Дата обнаружения:** 2026-10-27.
- **Компонент:** `tools/check_layout.py`, current Git worktree metadata and archived Stage 1/2 oracle report links.
- **Статус:** открыта; инфраструктурная проблема checkout, воспроизведена также при проверке S2-35.18.
- **Суть и последствия:** Layout validation reports the root `.git` worktree pointer as an unmapped file and two missing historical oracle manifests referenced by `docs/reports/STAGE_2_REPORT.md`. This prevents a clean layout-check result in this checkout; it does not affect interpreter or fixture execution.
- **Воспроизведение:** `python3 tools/check_layout.py` from the repository root in the linked Stage 2 worktree reports `Unmapped root file: .git` and broken local links to `work/stage2-oracle/stage2-20261022-230853-3684f650/manifest.json` and `work/stage1-oracle/stage1-20261022-230241-3b73e810/manifest.json`.
- **Следующий шаг:** update the layout checker to recognize Git worktree pointer files and restore the referenced manifests or correct their archived references in the report. Do not hide the failures by moving the checkout metadata or fabricating evidence.

### ISSUE-024 — S2-36 result APIs lacked large-gap rounding

- **Дата обнаружения:** 2026-10-28.
- **Компонент:** `od-isa` finite binary addition/subtraction result APIs `Binary32`/`Binary64`, S2-36o/p.
- **Статус:** решена для новых result APIs в S2-36p; старые low-level exact-alignment и legacy convenience APIs намеренно сохраняют bounded контракт gap≤64.
- **Суть и последствия:** до S2-36p новые result APIs возвращали `None` для конечных нормализованных операндов с exponent gap больше 64. Из-за этого полный finite диапазон локального add/sub result path не обрабатывался. Это было ограничением локального API, а не подтверждённым дефектом CPU и не определяло поведение x86-инструкций.
- **Историческое воспроизведение:** при прежней реализации gap 64 входил в bounded профиль, а gap 65 был первым отказом для result API. После S2-36p новые `add_finite_aligned_result`/`subtract_finite_aligned_result` обрабатывают gap до максимального конечного диапазона; literal-векторы проверяют gap 65 и 2097, все режимы округления, оба знака, операции и границы subnormal/overflow. Low-level `add_finite_aligned`/`subtract_finite_aligned` остаются bounded по контракту и не входят в закрытую проблему.
- **Результат:** private large-gap tail decomposition и round-equivalent bridge для precision 24/53 реализованы, независимо проверены и документированы в [плане S2-36p](plans/STAGE_2.md#s2-36p--finite-binary-addition-and-subtraction-result-apis-across-the-full-finite-exponent-range) и [отчёте S2-36p](reports/STAGE_2_REPORT.md#s2-36p--finite-binary-addition-and-subtraction-result-apis-across-the-full-finite-exponent-range). Авторские проверки: binary-float 48/48, x87 47/47, workspace offline check, format, diff check и Graphify update. Аппаратная/SSH регрессия отложена до общего финального substage и этим закрытием не заявляется.
- **Следующий шаг:** отсутствует для S2-36p result path. При отдельном расширении публичного low-level/legacy API потребуется явно определить новый контракт; их текущий лимит сам по себе не является открытой проблемой S2-36p.

### ISSUE-025 — Intel RCPSS/RCPPS threshold inequalities overlap

- **Дата обнаружения:** 2026-10-28.
- **Компонент:** нормативное описание legacy SSE binary32 reciprocal approximation; локальный reference helper S2-36ab и RCPSS/RCPPS profiles S2-37b/S2-37d/S2-37dy/S2-37ea.
- **Статус:** открыта; подтверждённая неоднозначность опубликованного текста, не подтверждённый runtime-дефект.
- **Основание и последствия:** Intel SDM Rev. 093, Vol. 2B, RCPPS pp. 4-538–4-539 and RCPSS pp. 4-540–4-541 describe threshold conditions for finite inputs/results, but the printed inequalities overlap on a numeric region. Thus the document does not yield a unique universal hardware cutoff around the normal/subnormal reciprocal-result boundary. The same entries state that the result is approximate with relative error at most `1.5 * 2^-12`, the operation is independent of MXCSR RC, denormal sources are treated as signed zero, tiny results are flushed to signed zero, SNaN is quieted, QNaN is returned, and no SIMD FP exceptions are signaled. The local S2-36ab helper therefore defines an explicit deterministic policy (exact same-format `1.0/source` rounded nearest-ties-even, then flush subnormal outputs), not a bitwise claim about hardware in the ambiguous region.
- **Воспроизведение/условие:** inspect the RCPPS and RCPSS threshold inequalities on the cited pages; compare their stated cutoff ranges numerically. The ambiguity affects expected hardware bits for normal inputs whose reciprocal lies near the normal/subnormal output transition. Local tests validate the stated error bound with integer-only products and do not assert a universal hardware cutoff.
- **Следующий шаг:** final Mac mini hardware acceptance should validate the `3/8192` relative-error envelope for non-flushed normal outputs, without requiring exact equality to the helper's bits. Treat the ambiguous tiny-result boundary as a separate unresolved question; resolve the overlapping inequalities against Intel errata or another authoritative clarification. Until then, retain the explicit local policy and do not claim a universal hardware cutoff or exact helper-bit match near the boundary. See [S2-36ab report](reports/STAGE_2_REPORT.md#s2-36ab--deterministic-raw-binary32-sse-reciprocal-reference), [S2-37b report](reports/STAGE_2_REPORT.md#s2-37b--local-only-long64-legacy-register-rcpss), [S2-37d report](reports/STAGE_2_REPORT.md#s2-37d--local-only-long64-legacy-register-rcpps), [S2-37ea report](reports/STAGE_2_REPORT.md#s2-37ea--rcpss-xmm0m32-rax), [S2-36ab plan](plans/STAGE_2.md#s2-36ab--deterministic-raw-binary32-sse-reciprocal-reference), [S2-37b plan](plans/STAGE_2.md#s2-37b--local-long64-register-only-legacy-rcpss), [S2-37d plan](plans/STAGE_2.md#s2-37d--local-long64-register-only-legacy-rcpps), and [S2-37ea plan](plans/STAGE_2.md#s2-37ea--local-long64-memory-legacy-rcpss-at-rax).

### ISSUE-026 — Plain Long64 FXSAVE/FXRSTOR target lacks selector state

- **Дата обнаружения:** 2026-10-29.
- **Компонент:** `od-isa::state::X87State`; plain Long64 `FXSAVE/FXRSTOR` in S2-39.
- **Статус:** resolved for independent selector representation and the bounded S2-39c/d save plus S2-39e restore profiles. Broader S2-39 forms and physical acceptance remain open outside this issue.
- **Суть и последствия:** `X87State` and strict schema-v2 `X87Snapshot` represent FCS/FDS independently as optional 16-bit selectors while preserving 64-bit FIP/FDP. `None` means unavailable/unrepresented; `Some(0)` is known zero. Plain `s2-long-fxsave-rip-v1` executes only unprefixed Long64 `0F AE 05 disp32` and requires both selectors plus x87/SSE state. The separate `s2-long-fxsave64-rax-disp8-v1` and `s2-long-fxrstor64-rbx-disp8-v1` profiles execute only exact forms `48 0F AE 40 40` and `48 0F AE 4B 40`; both accept selectors independently optional because the 64-bit image stores full FIP/FDP. Existing unrelated profiles still omit/reject selectors, and modeled x87 transitions fail closed when selector values are present. Guest exception delivery, broader addressing forms, and full system save/restore remain outside this issue.
- **Основание и воспроизведение:** S2-39b schema/profile tests cover selector availability and boundaries. S2-39c/d/e local tests cover pointer layouts, TOP rotation, abridged tags, XMM registers, immutable MXCSR policy, preserved/ignored image bytes, strict decode, callback order/failure and bounded round-trip where tag information is representable. Sources remain the selected target's leaf-7-zero rule and Intel SDM Rev. 093 Vol. 2A Tables 3-45..3-49 / Vol. 1 §§10.5.1..10.5.3; Ubuntu static mappings are in [S2-39a evidence](reports/STAGE_2_LINUX_PROFILE.md#s2-39a--fxsave-evidence-only).
- **Следующий шаг:** no further work is required to resolve selector representation for these bounded forms. Physical/Mac mini validation is deferred to the combined final hardware substage; no such result is claimed here. Broader S2-39 requirements remain tracked in the [Stage 2 plan](plans/STAGE_2.md#s2-39-сохранение-и-восстановление-fpsimd), [S2-39e contract](specs/CPU_CONTRACT.md#s2-39e--exact-long64-fxrstor64-rbx0x40), and [S2-39e report](reports/STAGE_2_REPORT.md#s2-39e--exact-long64-fxrstor64-rbx0x40).

### ISSUE-027 — First EFER access and startup control snapshot are unproven

- **Дата обнаружения:** 2026-10-29.
- **Компонент:** S2-00/S2-25 startup evidence for Ubuntu 7.0.0-38.38, IA32_EFER and initial CR0/CR4.
- **Статус:** открыта; это ограничение статического evidence, не подтверждённый дефект реализации.
- **Суть и последствия:** exact source/image mapping proves an EFER RDMSR and conditional WRMSR in the main-kernel direct64 `common_startup_64` continuation. It does not establish that this is the absolute first EFER access across all earlier compressed/decompression/`__pi_*` helpers, nor does static code reveal the actual EFER/CR0/CR4 entry snapshot. The nearby compiled CR0 load `0x80050033` includes AM and is not an entry snapshot; existing S2-25a uses a synthetic `0x80010033` base. Treating these as observed state or admitting EFER WRMSR as storage-only would overstate the evidence and omit mode/paging effects.
- **Основание:** authenticated source/archive hashes and image-derived/debug ELF mapping at `ffffffff8139c1b4` (`RDMSR`) and `ffffffff8139c1d5` (conditional `WRMSR`), with source `arch/x86/kernel/head_64.S:379–401`; see [S2-25d mapping](reports/STAGE_2_LINUX_PROFILE.md#ia32-efer-on-the-mapped-main-kernel-direct64-continuation).
- **Следующий шаг:** audit all earlier compressed/decompression and `__pi_*` helper paths for EFER accesses and capture/establish the relevant startup control-state assumptions through independent evidence. The S4-12a `from_cpu_state` bridge only validates an immutable diagnostic snapshot; it does not resolve startup provenance or WRMSR mode effects. Keep any next CPU slice read-only until WRMSR mode and paging effects have an owning contract; Stage 4 remains owner of system translation effects.
- **S2-25e:** implemented a local exact read-only `RDMSR IA32_EFER` fixture profile over four synthetic values with bounded current CR0/CR4 prerequisites. This does not change the open status: no actual Ubuntu entry value, initial CR snapshot, absolute-first-access ordering, WRMSR behavior, guest exception delivery, paging effect, or runtime boot observation is established. See [S2-25e CPU contract](specs/CPU_CONTRACT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer) and [implementation report](reports/STAGE_2_REPORT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer).

### ISSUE-053 — Stage 3 reports reference absent ignored evidence

- **Дата обнаружения:** 2026-11-04; разрешено в текущем main worktree 2026-11-04.
- **Компонент:** `docs/reports/STAGE_3*.md` and historical Stage 3 evidence under ignored `work/` paths.
- **Статус:** разрешена локальным восстановлением исходных файлов.
- **Суть и последствия:** Several Stage 3 reports linked to run summaries, manifests, and diagnostic attempts under `work/stage3-harness/` or `work/stage1-harness/` that were absent from this checkout. The reports could not be opened locally until those files were restored.
- **Основание и результат:** all 21 files reported as broken by `python3 tools/check_layout.py` were found in `/Users/senya/.codex/worktrees/stage3-baseline/open_desktop` and copied to the same relative `work/` paths in the main worktree. The layout checker now passes: `Layout OK: 48 required directories, 6 crate scaffolds, 14 instruction scopes.`
- **Граница:** `work/` remains Git-ignored, so these local evidence files are available in this main worktree but are not part of the source commit. If portable evidence is needed, curate the required records into `docs/reports/data/stage3/` under the project's evidence policy.

### ISSUE-058 — VMHost control race smoke fails on HEAD (Stage 10 session)

- **Дата обнаружения:** 2026-11-06, при проверке регрессии S10-06 и S10-07.
- **Компонент:** `tests/system/vmhost_control_race_smoke.py` с `apps/vmhost` и `apps/macos` launcher; не связан с дисплейным транспортом этапа 10.
- **Статус:** открыта. Воспроизводится на HEAD ветки `feat/s9-stage` без правок этапа 10 (два прогона у каждого варианта).
- **Суть и последствия:** скрипт завершается с `unsupported version: VMHost sent a response after rejecting the frame`. Причина не исследована; возможная регрессия control v1 или ошибка теста. Обычный путь VMHost по остальным `vmhost_*` smoke-скриптам проходит.
- **Основание:** запуски S10-06 и S10-07 на release-сборке; отдельный прогон на чистом HEAD.
- **Следующий шаг:** определить, какой из двух сторон (тест или VMHost) отклоняется от протокола control v1, и исправить соответствующую сторону; до этого не считать `vmhost_control_race_smoke.py` зелёным.

### ISSUE-059 — VMHost isolation smoke is intermittently red

- **Дата обнаружения:** 2026-11-06, при проверке S10-07.
- **Компонент:** `tests/system/vmhost_isolation_smoke.py`.
- **Статус:** открыта; причина не установлена.
- **Суть и последствия:** в 2 из 9 запусков скрипт завершился с кодом 1 (`surviving VMHost returned RAX=546, reason=2`); в остальных — код 0. Скрипт не использует launcher; файлы VMHost в этих коммитах не менялись, но сравнение с HEAD до S10-06 не выполнено.
- **Следующий шаг:** воспроизвести при повторных прогонах, найти условие гонки в тесте или в изоляции двух VMHost-процессов; до этого результат трактовать как нестабильный.

### ISSUE-060 — test_stage3_harness has seven failing count assertions

- **Дата обнаружения:** 2026-11-06, при проверке S10-07.
- **Компонент:** `tests/cpu/stage3_harness.py` (`python3 -m unittest test_stage3_harness` из `tests/cpu`).
- **Статус:** открыта; на чистом снимке HEAD падают те же 7 из 152 тестов (закреплённые счётчики, например `152 != 151`).
- **Суть и последствия:** инфраструктурный тест не отражает актуальный набор тестов; это не следствие этапа 10.
- **Следующий шаг:** обновить закреплённые счётчики или сами тесты в отдельной задаче по Stage 3 harness.

### ISSUE-061 — od-host JIT library tests are intermittently failing

- **Дата обнаружения:** 2026-11-06, при проверке S10-04.
- **Компонент:** `crates/od-host` lib tests (`s3_20*`, `native_register_v1_corpus`, ошибка `MappingAlreadyLive`).
- **Статус:** закрыта 2026-11-08; `cargo test -p od-host --lib --offline` проходит 361/361 как параллельно, так и с `--test-threads=1`.
- **Суть и последствия:** на HEAD `5acd5e5` падают 23 теста, при других прогонах 21; число меняется от прогона к прогону, что похоже на гонку при параллельном запуске. Новые display-тесты проходят. Причина не исследована.
- **Причина (2026-11-08):** это были не гонки, а детерминированные ошибки самих тестов, воспроизводимые с `--test-threads=1` и на коммите их появления `6588d50`. (1) `s3_20el/em/fg/fh` держали живой `PublishedCode` и публиковали второй при единственной допустимой `MAP_JIT` region (ISSUE-038), отсюда `MappingAlreadyLive`; (2) остальные `s3_20*` ожидали `GuestFault` для `Unmapped` либо `address != 0, length: 4` для `Backend`, тогда как `od-jit::memory_bridge::classify_error` возвращает `Err(MemoryError{kind, address, length})`, где `length` заполняется только для `AddressOverflow`, а `address` не заполняется для `Backend`; (3) `s4_15a/16a-16e` брали `JIT_TEST_LOCK.lock().unwrap()` и падали каскадом `PoisonError`; (4) `reduce_saved_native_register_v1_case` писал в фиксированный каталог с `create_new` и падал на втором запуске.
- **Исправление:** изменены только тесты. Вложенные публикации выполняются после `drop` предыдущего mapping и повторной публикации; ожидания приведены к фактической семантике моста с сохранением проверок состояния CPU, адреса, длины и счётчиков; poison-tolerant `jit_test_lock()`; reducer-тест использует каталог в temp с PID и удаляет его, если `OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR` не задан. Продакшен-код не менялся.
- **Остаётся:** правило заполнения `address`/`length` в `MemoryOperationMemoryErrorV2` тестами зафиксировано, но не описано в `docs/specs/JIT_CONTRACT.md`; отчёты Stage 9/10 (`STAGE_9_REPRO.md`, `STAGE_9_REPORT.md`, `STAGE_10_REPORT.md`) упоминают эти падения как известные и историческими не правились.

<a id="issue-055-workspace-check-invalidmovcr0lock"></a>

### ISSUE-055 — Workspace check does not handle the InvalidMovCr0Lock decode error

- **Дата обнаружения:** 2026-11-05.
- **Компонент:** `od-jit` decode-error classification in `block.rs` and `memory_dispatch.rs`.
- **Статус:** решена в S4-01b 2026-11-06.
- **Суть и последствия:** `cargo check --workspace --all-targets --offline` fails because two exhaustive matches do not cover `od_isa::long_decode::DecodeError::InvalidMovCr0Lock`. This blocks workspace-wide build validation; the affected cases are independent of the new `od-machine` mutable session.
- **Основание и результат:** the previously recorded exhaustive-match gap was resolved in both JIT consumers. `block::tests::lock_mov_cr0_is_an_exact_unsupported_boundary_and_preserves_progress` verifies exact RIP, typed `InvalidMovCr0Lock`, length 4, and preserved decoded prefix; `memory_dispatch::tests::lock_mov_cr0_stops_strict_dispatch_without_retirement_or_tail_execution` verifies strict `Unsupported`, zero retirement at entry, and stopping after the preceding supported memory artifact without executing the suffix. The LOCK MOV CR3 regression also passes. Fresh `cargo check --workspace --all-targets --offline` completed with exit code 0. Details and commands: [S4-01b report](reports/STAGE_4_S4-01B_2026-11-06.md) and [manifest](reports/data/stage4/s4-01b-invalid-mov-cr0-lock-2026-11-06.json).
- **Граница:** this closes only the missing `InvalidMovCr0Lock` classification in `od-jit`. JIT/native admission was not widened; the parent S4-01 baseline and Stage 4 remain open.

<a id="issue-062-cargo-fmt-check-fails-in-unrelated-files"></a>

### ISSUE-062 — `cargo fmt --all -- --check` fails in files outside stage 11

- **Дата обнаружения:** 2026-11-06, при проверке S11-07.
- **Компонент:** ранее отмеченные `crates/od-host/src/display.rs`, `crates/od-machine/src/lib.rs` и `tools/power-loss/src/main.rs`; на 2026-11-14 также `crates/od-host/src/storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs`.
- **Статус:** открыта; проверка форматирования, не runtime-дефект.
- **Суть и последствия:** на HEAD `cargo fmt --all -- --check` возвращает exit 1 из-за неотформатированных файлов, которые не относятся к задаче S11-07. Новый код этапа 11 проверяется `rustfmt --check` по своим файлам и проходит.
- **Основание:** вывод `cargo fmt --all -- --check` на worktree `feat/s9-stage` после S11-07 (exit 1); повтор 2026-11-14 в `codex` (exit 1; diff в `storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs`). Затронутые файлы не относятся к Stage 5 изменениям.
- **Следующий шаг:** отдельным change-only-формат коммитом привести перечисленные файлы к `rustfmt`, не смешивая с функциональными правками; после этого вернуть полный `cargo fmt --all -- --check` в обязательные проверки.

<a id="issue-056-layout-check-unmapped-smoke-root"></a>

### ISSUE-056 — Layout checker rejects the root smoke directory

- **Дата обнаружения:** 2026-11-05.
- **Компонент:** repository root layout and `tools/check_layout.py`.
- **Статус:** открыта; verified repository-layout failure, not a runtime defect.
- **Суть и последствия:** the main checkout contains a root `smoke/` directory that is not mapped by the project layout, so the layout check exits unsuccessfully. The directory was left untouched as requested; this slice does not change its ownership or contents.
- **Основание:** running `python3 tools/check_layout.py` from `/Users/senya/projects/open_desktop` returned `ERROR: Unmapped root directory: smoke` (exit code 1).
- **Следующий шаг:** determine the intended owner and purpose of root `smoke/`, then update the project layout or directory placement in a separately scoped change. Do not silently remove or remap it as part of S4-14c.

<a id="issue-076-native-v2-linux-e0063"></a>
### ISSUE-076 — `tools/x86-runner/src/native_v2.rs` does not compile on Linux (E0063)

- **Дата обнаружения:** 2026-11-12.
- **Компонент:** `tools/x86-runner/src/native_v2.rs`, test helper `fn state` (около строки 2066).
- **Статус:** открыта; pre-existing, не вносится срезами этапа 4.
- **Суть и последствия:** под Linux x86_64 крейт `od-x86-runner` не собирается с `E0063` (missing fields `cr2`, `fs_unusable`, `gs_unusable` и др. в инициализаторе `LongModeState`): поля добавлены позже, чем последний раз менялся этот модуль (2026-11-05). Поэтому `native_v2::admission` не запускается ни на одной платформе; на macOS `od-x86-runner` проходит только за счёт `cfg(target_os = "linux")`. Срезы S4-38L7ai…ap проверяли admission только по чтению кода.
- **Основание:** `cargo check --target x86_64-unknown-linux-gnu -p od-x86-runner --tests` (агент S4-38L7ai, ревью Opus S4-38L7ai).
- **Следующий шаг:** в отдельном срезе обновить тестовый конструктор `state` под текущие поля `LongModeState` и прогнать `native_v2` admission на Linux x86_64. До этого не считать admission проверенным для новых операций.

<a id="issue-077-kernel-gs-base-two-stores"></a>
### ISSUE-077 — значение IA32_KERNEL_GS_BASE хранится в двух местах

- **Дата обнаружения:** 2026-11-12.
- **Компонент:** `od-isa` (`state.kernel_gs_base`, профиль `KernelGsBaseV1`, SWAPGS-профиль) и набор CPL0 / плата (`GuestMsrCells::kernel_gs_base`, `guest_kernel_gs_base`), срез S4-38L7au.
- **Статус:** открыта; архитектурный долг, не влияет на текущий прогон (SWAPGS в наборе не реализован).
- **Суть и последствия:** член набора `msr_kernel_gs_base` пишет в собственную ячейку, а профиль `KernelGsBaseV1` (RDMSR/WRMSR/SWAPGS) — в `state.kernel_gs_base`. При выбранном профиле член не действует (профиль побеждает), поэтому два хранилища не расходятся сегодня. Но SWAPGS в наборе CPL0, реализованный без объединения, сделает значение двоящимся: SWAPGS будет читать и менять `state.kernel_gs_base`, а гостевой WRMSR — ячейку.
- **Основание:** ревью Opus S4-38L7au; постановка срезов S4-38L7aq…au (ячейки по образцу STAR/LSTAR).
- **Следующий шаг:** при реализации SWAPGS в наборе CPL0 связать ячейку члена с `state.kernel_gs_base` (или сделать ячейку единственным хранилищем набора), покрыть тестом цепочку WRMSR → SWAPGS → RDMSR на одной плате.

### ISSUE-078 — PIT отказывает в команде read-back (`0xC2` на порт `0x43`), прогон Linux останавливается

- **Дата обнаружения:** 2026-11-12.
- **Компонент:** `crates/od-machine/src/pit_8254.rs` (`write_control`, `UnsupportedConfiguration`); порт `0x43` в плате Linux (`device_index` 6, `firmware_board.rs`).
- **Статус:** открыта; блокирует продвижение Linux-загрузки после S4-38L7bk. Не ошибка CPU: инструкция `out %al, $0x43` исполняется.
- **Суть и последствия:** ядро 7.0.0-38 по RIP `0xffffffff8271ea4c` пишет в порт `0x43` управляющее слово `0xC2` (биты 7:6 = `11`, команда read-back), затем читает `0x40`. Модель PIT отказывает в read-back до любых изменений состояния, поэтому прогон останавливается на `PortIo(Device { … UnsupportedConfiguration })`. Семантику (какие байты состояния и счётчика возвращаются, защёлка, порядок чтений) по таблицам по памяти определить нельзя; без решения её не реализовать.
- **Основание:** диагностический прогон S4-38L7bk (`docs/reports/data/stage4/s4-38l7bk-diagnostic-run.json`, `diagnostic_not_counted`); ревью Opus S4-38L7bk.
- **Следующий шаг:** решение владельца этапа по семантике read-back; затем отдельный срез устройства в `pit_8254.rs` с тестами и без правок CPU-набора.

<a id="issue-082-s2-37t-review-status-provenance-disagrees"></a>
### ISSUE-082 — S2-37t review-status provenance disagrees between plan and report

- **Дата обнаружения:** 2026-11-13.
- **Компонент:** `docs/plans/STAGE_2.md` summary row S2-37t and `docs/reports/STAGE_2_REPORT.md` S2-37t review status.
- **Статус:** открыта; документальная inconsistency, не подтверждённый CPU-дефект.
- **Суть и последствия:** plan summary labels the independent Sol-medium review clean, while the S2-37t report body says the independent code review is pending. A separate read-only review of HEAD `9d5fb75` confirmed only that the implemented bounded semantic/state predicates make S2-37t eligible as a conditional-local inventory candidate. It did not rerun tests and does not resolve the historical full-code-review status.
- **Основание:** conflicting status text in the two Stage 2 documents; the limited S5-21 eligibility review is recorded in the report and candidate metadata.
- **Следующий шаг:** reconcile the historical review claim from its original review evidence; until then preserve the conflict and do not cite the limited candidate-eligibility review as a full code review.

<a id="issue-083-stage4-leave-sdm-source-unverified"></a>

### ISSUE-083 — S4-38L6d LEAVE: SDM attribution has not been verified

- **Дата обнаружения:** 2026-11-13, при статической разметке S5-21 кандидата `C9`.
- **Компонент:** Stage 4 LEAVE CPL0 profile; source provenance for x86 `LEAVE` semantics.
- **Статус:** открыта; это непроверенная атрибуция источника, не подтверждённый дефект реализации.
- **Суть и последствия:** [LEAVE contract, CPU_CONTRACT.md:21161–21169](specs/CPU_CONTRACT.md#L21161) прямо сообщает, что ссылка на Intel SDM Vol. 2 приведена по памяти, текст первоисточника не приложен и не сверялся. Поэтому текущий source attribution нельзя считать independently source-verified. S5-21 добавил только conditional candidate metadata и не подтверждает CPU semantics. LEAVE читает qword stack value по RBP; это memory access и остаётся частью профиля.
- **Основание:** LEAVE в [CPU_CONTRACT.md:21161](specs/CPU_CONTRACT.md#L21161); Stage 4 plan aggregate [line 2150](plans/STAGE_4.md#L2150); S5-21 [report follow-up](reports/STAGE_5_REPORT.md#s5-21-cpl0-conditional-candidate-follow-up-pause--leave-2026-11-13). PAUSE profile находится в [CPU_CONTRACT.md:24347](specs/CPU_CONTRACT.md#L24347), с агрегированной ссылкой в Stage 4 plan [line 2078](plans/STAGE_4.md#L2078).
- **Следующий шаг:** Stage 4 owner сверяет LEAVE encoding, 64-bit semantics и exception behavior с применимой редакцией Intel SDM и обновляет S4-38L6d / CPU contract с точной ссылкой; до этого сохранять заявленную границу provenance.

- **Latest compact annotation (2026-11-13, after bounded PREFETCH candidates):** rootfs SHA-256 `beb92e83d2c02c17dd50a9a616aad298a79bf42ca9f7db32cdf2b970f1f1eec4`; E-inclusive SHA-256 `2d332123379c6bf55a065ebbcb317fbe77a6dc4c111444337df05da118c71066`. Raw analyzer pair and source inventory are unchanged; no new bytewise raw rerun is claimed. Current candidate totals and limits are recorded in the [S5-21 report](reports/STAGE_5_REPORT.md#s5-21-cpl0-candidate-follow-up-prefetchnta--prefetcht0-2026-11-13).

<a id="issue-084-stage4-prefetch-review-minor-tests-open"></a>

### ISSUE-084 — S4-38L6a review minor tests remain open

- **Дата обнаружения:** 2026-11-13, при S5-21 PREFETCH candidate follow-up.
- **Компонент:** Stage 4 `0F 18 /0..7 m` PREFETCH CPL0 profile and `cpl0_profile_set` tests.
- **Статус:** открытые minor verification items из принятого review; подтверждённого дефекта нет.
- **Суть и последствия:** Stage 4 plan records missing tests for a truncated operand and repeated `66`. S5-21 only annotated the observed exact bare base-only forms `0F 18 00` and `0F 18 08`; it does not cover decoder rejection, execution behavior for truncated input, or repeated-prefix behavior.
- **Основание:** [S4-38L6a plan entry](plans/STAGE_4.md#L2152) and the bounded [S5-21 PREFETCH report](reports/STAGE_5_REPORT.md#s5-21-cpl0-candidate-follow-up-prefetchnta--prefetcht0-2026-11-13).
- **Следующий шаг:** add focused truncated-operand and repeated-`66` tests to the Stage 4 profile and review that boundary; keep the candidate inventory limited to exact observed forms until then.

<a id="issue-085-stage4-prefetch-sdm-source-unverified"></a>

### ISSUE-085 — S4-38L6a PREFETCH SDM attribution has not been verified

- **Дата обнаружения:** 2026-11-13, при статической разметке S5-21 PREFETCH candidates.
- **Компонент:** Stage 4 PREFETCHNTA/PREFETCHT0 CPL0 profile; source provenance for `0F 18 /0..7 m` semantics.
- **Статус:** открыта; это непроверенная атрибуция источника, не подтверждённый дефект реализации.
- **Суть и последствия:** [CPU_CONTRACT.md:21069–21071](specs/CPU_CONTRACT.md#L21069) attributes the PREFETCH semantics to the Intel SDM from memory without an independently checked source. S5-21 added candidate metadata only and does not verify source provenance or establish architecture-wide semantics.
- **Основание:** [S4-38L6a plan entry](plans/STAGE_4.md#L2152) and the [S5-21 PREFETCH report](reports/STAGE_5_REPORT.md#s5-21-cpl0-candidate-follow-up-prefetchnta--prefetcht0-2026-11-13).
- **Следующий шаг:** verify the applicable Intel SDM edition and record its exact citation in the CPU contract and Stage 4 result; preserve the current provenance boundary until verified.

<a id="issue-086-s9-26-wrong-uefi-stage-reference"></a>

### ISSUE-086 — S9-26 names the wrong UEFI stage

- **Дата обнаружения:** 2026-11-13, при подготовке S6-00.
- **Компонент:** `docs/reports/STAGE_9_REPORT.md`, запись S9-26.
- **Статус:** открыта; подтверждённая документальная ошибка, не дефект реализации.
- **Суть и последствия:** запись S9-26 указывает, что UEFI firmware отсутствует на этапах 6/8. UEFI принадлежит этапу 7. Текущая формулировка ошибочно распределяет владельца и может привести к неправильному планированию блокеров.
- **Основание:** строка S9-26 в отчёте этапа 9 сопоставлена с планом этапа 7 и картой этапов; та же находка описана в [отчёте этапа 6](reports/STAGE_6_REPORT.md#4-документные-расхождения-и-передача).
- **Следующий шаг:** владелец этапа 9 исправляет ссылку на этап 7 в `STAGE_9_REPORT.md`; до исправления сохранять эту запись открытой.

### ISSUE-087 — цикл CPL3-входа продолжает исполнять код ядра после SYSCALL

- **Дата обнаружения:** 2026-11-14, ревью S4-39e.
- **Компонент:** `crates/od-isa/src/execute.rs`, `run_with_cpl3_profile_set` (цикл по бюджету).
- **Статус:** закрыта в S4-39f. Цикл `run_with_cpl3_profile_set` после шага, оставившего CPL3 (CS RPL ≠ 3 или `ring3_state_admitted` ложно), при оставшемся бюджете выходит с `Run(StopAddress)` до следующей выборки; тест `the_cpl3_loop_stops_at_the_cpl_change_of_syscall` (бюджет 3), раздел S4-39f в `docs/specs/CPU_CONTRACT.md`. До исправления: на стенде недостижима (плата передаёт бюджет 1), в исполнении — латентная ошибка.
- **Суть и последствия:** после `SYSCALL` CS RPL становится 0, но цикл не останавливается. При бюджете больше 2 следующая инструкция ядра декодируется и исполняется с контекстом Ring3 и белым списком: разрешённые формы исполнятся «как CPL3», выборка со страницы супервизора даст ложный #PF с U=1, иначе `Unsupported` с опкодом ядра.
- **Основание:** ревью S4-39e (Opus); тест `syscall_follows_the_init_prefix_in_the_same_run` берёт бюджет ровно 2 и случай не видит.
- **Следующий шаг:** нет (исправлено в S4-39f: выход из цикла при CS RPL ≠ 3 или `ring3_state_admitted` = false, тест с бюджетом 3).

### ISSUE-088 — счётчики syscall-журнала занижаются при больших квантах платы

- **Дата обнаружения:** 2026-11-14, ревью S4-39f.
- **Компонент:** `crates/od-machine/src/paged_memory_adapter.rs` (`SYSCALL_EVENTS_PER_CALL`), `crates/od-machine/src/linux_direct.rs` (`SyscallJournal`).
- **Статус:** открыта; на стенде не проявляется (один вход SYSCALL), проявится в S4-39g и далее.
- **Суть и последствия:** события одного вызова адаптера ограничены 64 и вытесняются из очереди; `entered`/`returned` считаются по уцелевшим событиям. При квантах больше 32 пар SYSCALL/SYSRET счётчики занижены; осиротевший Exit может привязаться к чужой незакрытой записи.
- **Следующий шаг:** счётчики вести в отчёте платы отдельно от очереди; при вытеснении отбрасывать Exit без пары. Срок — до S4-39g; S4-39g принят (`2ddec59`), срок не закрыт: ISSUE-088 переносится на S4-39j (гейт), пока счётчики журнала при больших квантах не проверены прогоном.
- **Обновление S4-46 (2026-11-15):** остаётся открытой. В зачётном прогоне S4-39j журнал совпал с ожидаемой последовательностью (9 записей, `entered` 9, `returned` 8, ёмкость 32), занижения не наблюдалось, но сценарий не содержит более 32 пар SYSCALL/SYSRET за квант, поэтому проверка при больших квантах не выполнена и гейт её не делает. Владелец: этап 5 (устойчивые workloads) либо срез гейта (см. ISSUE-098); срок «до S4-39j» не закрыт, а снят с S4-39j и не назначен.

<a id="issue-089-s11-12-sleep-pause-refusal-not-persisted"></a>

### ISSUE-089 — отказ системной PAUSE не сохраняется в журнале

- **Дата обнаружения:** 2026-11-06; повторно подтверждено независимым review S11-12 2026-11-14.
- **Компонент:** `apps/macos/Sources/App/MachineController.swift`, обработка отказа `PAUSE reason=system_sleep`.
- **Статус:** решена 2026-11-14; независимое повторное ревью не нашло blocker/major/minor замечаний.
- **Результат:** отказ `PAUSE reason=system_sleep` отправляется в unified logging с subsystem `dev.opendesk.OpenDesktop`, категорией `MachineLifecycle` и только публичным числовым кодом ошибки. Имя машины, пути и текст диагностического сообщения не записываются. Уведомление по-прежнему показывается после пробуждения. Тест проходит через отказ запроса, проверяет аудит-код и уведомление после wake; `swift test --package-path apps` — 344 tests, 0 failures.
- **Проверка и документация:** [MachineController.swift](../apps/macos/Sources/App/MachineController.swift), [тесты S11-12](../apps/macos/Tests/HostConditionsTests.swift), [отчёт этапа 11](reports/STAGE_11_REPORT.md). Локальные файлы в `logs/` bundle не используются, поэтому владельцем каталога и ротацией по-прежнему остаётся VMHost согласно [VM_BUNDLE §2.2](specs/VM_BUNDLE.md#22-logs).
