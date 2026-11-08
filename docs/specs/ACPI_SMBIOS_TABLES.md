# Таблицы ACPI и SMBIOS из handoff block

**Статус:** документ создан задачей S7-12 [плана этапа 7](../plans/STAGE_7.md#s7-12-реализовать-собственный-aml-кодировщик-и-dsdt-из-manifest) и сейчас содержит только раздел «AML и DSDT» (выполнено исполнителем, ожидает ревью). Разделы по RSDP/XSDT/FADT/MADT/MCFG/HPET/TPM2/FACS и SMBIOS пишет S7-13; их здесь нет, и ни одна из этих таблиц не реализована. DSDT не установлена как configuration table и не проверялась в машине или ОС: это функции над байтами с host-side тестами (уровень 1, [FIRMWARE.md](../testing/FIRMWARE.md)).

Связанные документы: [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §6 (handoff block v1.0 — единственный источник данных), [виртуальная плата](VIRTUAL_BOARD.md), [правила каталога прошивки](../../firmware/AGENTS.md), [README прошивки](../../firmware/uefi/README.md).

## AML и DSDT

### Источники и редакции

- ACPI Specification, главы 20 «ACPI Machine Language (AML) Specification» (грамматика 20.2.2–20.2.5, Table 20.2), 6.4 «Resource Data Types for ACPI» (Tables 6.28, 6.33, 6.37, 6.43, 6.46, 6.47, 6.49, 6.52), 6.2.13 (`_PRT`), 19.3.4 (EISAID), 5.2.6 и 5.2.11.1 (заголовок таблицы, DSDT, Tables 5.4 и 5.17).
- **Редакция фактически прочитанного текста — ACPI 6.4** (HTML-копия `uefi.org/htmlspecs/ACPI_Spec_6_4_html`, полученная через Context7, библиотека `/websites/uefi_htmlspecs_acpi_spec_6_4_html`, 2026-11-07). Страницы ACPI 6.6, на которые ссылается [VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) («Нормативные источники и редакции»), на `uefi.org/specs/ACPI/6.6/…` вернули HTTP 403 (curl и WebFetch). Совпадение указанных таблиц и кодировок в 6.6 **не проверено**.
- Чужие `.asl`/`.aml`, DSDT QEMU/OVMF/ПК и исходники других кодировщиков не использовались. Опкоды и поля взяты из таблиц спецификации; у каждой константы в `firmware/uefi/src/acpi/aml/opcodes.rs` указан раздел или таблица.
- Эталонные байты в тестах вычислены вручную по спецификации (конструкты) и независимой Python-программой по тексту ACPI 6.4 (DSDT); скрипт не сохранён, общего кода с Rust у него нет. `iasl` на host отсутствует и не использовался: пункт «разбор `iasl -d` как диагностика» **не сделан**.

### Кодировщик (`firmware/uefi/src/acpi/aml/`)

Кодировщик, а не интерпретатор. Работает без `alloc` в буфере вызывающего. Модули:

| Модуль | Содержимое |
|---|---|
| `opcodes.rs` | значения опкодов, префиксов и тегов дескрипторов с ссылкой на раздел/таблицу |
| `names.rs` | `NameSeg` (1–4 символа, `[A-Z_][A-Z0-9_]*`, добивка `_`), `AmlName` (`\`, `^`, до 8 сегментов), `AbsPath`, `Registry` (имена объектов по абсолютному пути) |
| `encoder.rs` | PkgLength (1–4 байта, включающая длина, до `2^28 − 1`), NameString (`NullName`, `NameSeg`, `0x2E`, `0x2F`), Integer (`Zero/One/Byte/Word/DWord/QWord` — наименьшая форма), String, закрытие пакета с подстановкой длины |
| `resource.rs` | `ResourceTemplate`: `Memory32Fixed`, `IO`, `IRQ` (короткая и длинная форма), `Interrupt` (Extended Interrupt), `WordBusNumber`, `DWordMemory`, `QWordMemory`, End Tag |
| `builder.rs` | `Scope`, `Device`, `Method` (без логики или с `Return(integer)`), `Name` (+ `Integer/String/EISAID/ResourceTemplate/Package`), `Package`, `Buffer`, ссылка на определённый объект; повтор имени в одном Scope → `DuplicateName` |
| `validate.rs` | структурный проход по готовому AML: проверяет **только** границы пакетов и повторы имён в одном Scope; неизвестный опкод — ошибка. **Не разбирает** содержимое `Buffer`/`ResourceTemplate`, `NumElements` у `Package` и тело `Method` (это объём п. 3 плана S7-12) |

Объекты `_HID/_CID/_UID/_CRS/_STA/_PRT/_ADR/_BBN/_SEG` строятся как обычные `Name` (`Name(name, значение)`); отдельного типа для них нет.

Ограничения: ссылки в `Package` — только абсолютные и только на уже определённые объекты; `Method` не регистрирует объекты внутри тела; `ResourceTemplate` вмещает 256 байт; глубина вложенности и число именованных объектов ограничены (8 и 64); Resource Source в дескрипторах не пишется.

### DSDT из manifest (`firmware/uefi/src/acpi/dsdt.rs`, `manifest.rs`, `table.rs`)

`BoardManifest` — семь записей таблицы устройств handoff block v1.0 (FIRMWARE_BOOT.md §6.3, 48 байт: `type`, `flags`, `base`, `length`, `gsi`, `irq_flags`, `param0`, `param1`); других полей нет. Проверки: каждый тип 1–7 ровно один раз (в любом порядке), зарезервированные биты `flags` и `irq_flags` — нули, у `PRESENT = 0` нет адреса. Разбор заголовка и таблицы регионов handoff block — задача ядра прошивки (S7-04), здесь не дублируется.

DSDT: заголовок 36 байт (Table 5.17: сигнатура `DSDT`, ревизия 2, `OEMID` `OPNDSK`, `OEM Table ID` `ODFWDSDT`, `OEM Revision` 1, `Creator ID` `ODFW`, `Creator Revision` 1 — собственные строки, S7-13 берёт их общими для всех таблиц), затем AML `Scope(\_SB) { … }`. Дочерние устройства выводятся только при `PRESENT = 1`, в порядке PCI0, COM1.

| Устройство в DSDT | Источник в manifest | Содержимое |
|---|---|---|
| `\_SB.PCI0` | запись типа 7 (PCIe ECAM), `PRESENT = 1`, не PIO | `_HID` `PNP0A08`, `_CID` `PNP0A03`, `_UID` 0, `_SEG` = `param0` (до `0xFFFF`), `_BBN` = `start_bus` (`param1` биты 7..0), `_CRS`: один `WordBusNumber` (ResourceProducer, MinFixed, MaxFixed) `start_bus..=end_bus` (`param1` биты 15..8, `start ≤ end`, биты 31..16 нули), `_PRT`: пустой пакет |
| `\_SB.COM<n>` | запись типа 1 (COM1), `PRESENT = 1`, `PIO` | имя `COM` + цифра `param0` (1–9), `_HID` `PNP0501`, `_UID` = `param0`, `_CRS`: `IO(Decode16, base, base, 1, length)` (`base` и `length` из записи; `base + length ≤ 0x10000`, `1 ≤ length ≤ 255`) и, если `gsi ≠ 0xFFFF_FFFF`, один `Interrupt(ResourceConsumer, …) {gsi}`; режим и полярность из `irq_flags` (бит 0: 1 = level, 0 = edge; бит 1: 1 = active-low), Exclusive |

Для firmware board v1 COM1 даёт `IO 0x3F8 length 8` и `Interrupt(ResourceConsumer, Edge, ActiveHigh, Exclusive) {4}` (IRQ4 по FIRMWARE_BOOT.md §6.3; доставки IRQ4 нет, прошивка опрашивает COM1 — это описание ресурса, а не обещание работы прерывания в гостевой ОС).

У `\_SB.PCI0` нет `_ADR`: у устройства должен быть `_HID` или `_ADR`, не оба (исправлено по ревью S7-12; эталон `both` пересчитан).

**Пробел контракта (не выдумано).** Manifest v1.0 не содержит окон I/O и MMIO корневого моста PCIe и маршрутов прерываний PCI. Поэтому `_CRS` PCI0 содержит только диапазон шин, а `_PRT` пуст (ACPI 6.2.13 допускает пустой `_PRT`: маршрутов нет; текст 6.2.13 не сверен). Такая PCI0 не позволит ОС назначать BAR; окна и маршруты должны появиться в handoff block (повышение `version_minor`, S4-30/S7-13), после чего DSDT получит `DWordMemory`/`QWordMemory`/`IO` из них. Записано в [KNOWN_ISSUES.md](../KNOWN_ISSUES.md#issue-065-s7-dsdt-pci0-has-no-io-mmio-windows-or-interrupt-routes). Запись ECAM имеет `PRESENT = 0` до S4-30, поэтому до него PCI0 в DSDT не выводится.

### Отложено

| Что | Почему не здесь | Кто делает |
|---|---|---|
| RTC `PNP0B00`, HPET `PNP0103` | в manifest v1.0 нет их записей (коды 8–15 зарезервированы под S7-14); устройств на плате нет | S7-14 |
| TPM `MSFT0101`, `_CRS` CRB `0xFED4_0000` | запись типа 6 `PRESENT = 0` до S7-22; CRB-устройства на плате нет | S7-22 |
| `_S5_` (выключение через PM1) | нужен ACPI fixed hardware | S7-14 |
| Local APIC, IOAPIC, дисплей, NVRAM в DSDT | описываются таблицами MADT и конфигурацией ОС, а не AML; отдельного решения «описывать ли в DSDT» нет | S7-13 (состав — после S7-30) |
| Установка DSDT как configuration table, `X_DSDT` в FADT | остальные таблицы — S7-13 | S7-13 |

Механизм «устройство есть в manifest» реализован для PCI0 и COM1; для отложенных устройств добавление означает новую запись manifest и новый вывод в `dsdt.rs`, а не изменение кодировщика.

### Валидатор

`validate_dsdt` (`dsdt.rs`) проверяет: длину таблицы (поле `Length` равно числу байтов), сигнатуру, контрольную сумму (Table 5.4/5.17: вся таблица, включая поле контрольной суммы, в сумме по модулю 256 даёт нуль), затем `validate_definition_block`: границы каждого пакета внутри родительского и отсутствие повторяющихся имён объектов в одном Scope. `build_dsdt` вызывает его для собственного результата.

### Что не сверено с текстом спецификации

- Все значения — по ACPI 6.4, не по 6.6 (см. выше).
- `ReturnOp` `0xA4`: часть Table 20.2 выше `0x74` источник не вернул. Используется только `Return(integer)` в `Method` (в DSDT методов нет; опкод проверен лишь тестом по значению).
- QWord Address Space Descriptor: тег `0x8A`, длина 43 и порядок полей взяты по аналогии с проверенными Word (13) и DWord (23); Table 6.45 источник не вернул. В DSDT не используется.
- Грамматика `DefBuffer := BufferOp PkgLength BufferSize ByteList` и `DefPackage := PackageOp PkgLength NumElements …` не получена дословно; подтверждены фиксированные аргументы в Table 20.2 и правило 5.4.1 (все объекты с явной длиной начинаются с PkgLength).
- Type Specific Flags для диапазона номеров шин пишутся нулём; таблица флагов для `Resource Type = 2` не получена (для памяти Table 6.49 проверена).
- General Flags бит 0 дескрипторов адресного пространства Table 6.46/6.47 называет «Ignored»; пишется 0 (ResourceProducer), соответствие ASL `ResourceUsage` не проверено.
- Контрольная сумма End Tag пишется нулём («0 — проверка считается успешной», Table 6.37); правило вычисления ненулевой суммы макросом `ResourceTemplate` не проверено.
- Идентификаторы `PNP0A08` (PCIe root bridge) и `PNP0501` (UART 16550) взяты из плана S7-12; в тексте спецификации ACPI они не найдены (реестр PNP/ACPI ID — внешний, `uefi.org/PNP_ACPI_Registry`). `PNP0A03` как PCI root bridge подтверждён примерами ACPI 6.4; `PNP0103` и `PNP0B00` (отложены) не проверялись.
- Поведение Windows/Linux при чтении этой DSDT не проверялось.

### Проверка

`cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — модули `acpi::aml::tests` (конструкты, отрицательные случаи), `acpi::dsdt::tests` (эталонные DSDT побайтно, вариации manifest, повреждения), `acpi::manifest::tests`, `acpi::table::tests`. Результат — в [отчёте этапа 7](../reports/STAGE_7_REPORT.md#s7-12-aml-кодировщик-и-dsdt-из-manifest).
