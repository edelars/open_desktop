# Таблицы ACPI и SMBIOS из handoff block

**Статус.** S7-12 определяет AML и DSDT. S4-31 определяет freestanding blob v2. S7-13 использует тот же сериализатор для публикации ACPI из handoff и публикует структуры SMBIOS 3.9 через UEFI System Table. Условия проверки описаны в [плане этапа 7](../plans/STAGE_7.md#s7-13-построить-таблицы-acpi-и-smbios-из-handoff-block).

Раздел «Состав ACPI-таблиц для Linux-пути этапа 4 (S4-31a)» добавлен срезом S4-31a этапа 4 (состав таблиц и сериализатор MCFG).

Связанные документы: [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §6 (handoff block v1.0 — единственный источник данных), [виртуальная плата](VIRTUAL_BOARD.md), [правила каталога прошивки](../../firmware/AGENTS.md), [README прошивки](../../firmware/uefi/README.md).

## AML и DSDT

### Источники и редакции

- Исходные ссылки ACPI 6.4. Главы 20 «ACPI Machine Language (AML) Specification» (грамматика 20.2.2–20.2.5, Table 20.2), 6.4 «Resource Data Types for ACPI» (Tables 6.28, 6.33, 6.37, 6.43, 6.46, 6.47, 6.49, 6.52), 6.2.13 (`_PRT`), 8.4 (processor device object), 19.3.4 (EISAID), 5.2.6, 5.2.11.1 и 5.2.12.2 (заголовок таблицы, DSDT, MADT Local APIC).
- Исходные эталоны 2026-11-07 получены по ACPI 6.4. Выборочная сверка с ACPI 6.6 и область покрытия описаны ниже.
- Чужие `.asl`/`.aml`, DSDT QEMU/OVMF/ПК и исходники других кодировщиков не использовались. Опкоды и поля взяты из таблиц спецификации; у каждой константы в `firmware/uefi/src/acpi/aml/opcodes.rs` указан раздел или таблица.
- Эталонные байты в тестах вычислены вручную по спецификации (конструкты) и независимой Python-программой по тексту ACPI 6.4 (DSDT); скрипт не сохранён, общего кода с Rust у него нет. Для S4-31 prerequisite итоговая DSDT дополнительно разобрана ACPICA `iasl` версии 20260408; это проверка синтаксиса AML, не установка или поведение таблиц в ОС.

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

DSDT: заголовок 36 байт (Table 5.17: сигнатура `DSDT`, ревизия 2, `OEMID` `OPNDSK`, `OEM Table ID` `ODFWDSDT`, `OEM Revision` 1, `Creator ID` `ODFW`, `Creator Revision` 1 — собственные строки, S7-13 берёт их общими для всех таблиц), затем AML `Scope(\_SB) { … }`. Объект `\_S5_` добавляется вариантом `build_dsdt_with_s5` (S4-31e, см. таблицу ниже); он не добавляет устройств. Для фиксированного одноядерного профиля первым всегда объявляется `Device(CPU0)` с `_HID` строкой `ACPI0007` и целочисленным `_UID` 0. UID совпадает с единственной Processor Local APIC записью MADT (S4-31f); это обязательная пара по ACPI 6.4 §5.2.12.2, а формат Device/HID/целочисленного UID следует §8.4 и правилу ACPI 6.3+. Далее manifest-устройства выводятся при `PRESENT = 1`, в порядке PCI0, ECAM, COM1. Запись ECAM проверяется правилами MCFG (S4-31a): окно в памяти (не PIO), сегмент 0, шины 0..=0, длина `(end_bus − start_bus + 1) << 20`, база ненулевая, кратная 1 MiB, конец не выше 4 GiB, `param1` без битов выше 15; иначе `build_dsdt` возвращает `DsdtError::Ecam(McfgError)`. Прежняя тестовая конфигурация S7-12 (база `0xC000_0000`, 256 MiB, шины 0..=0xFF) вне контракта платы и не собирается.

| Устройство в DSDT | Источник в manifest | Содержимое |
|---|---|---|
| `\_SB.CPU0` | фиксированный профиль S4-31/S4-31f (не отдельная запись handoff) | `_HID` — строка `ACPI0007`, `_UID` — целое `0`; совпадает с UID единственной Local APIC записи MADT |
| `\_SB.PCI0` | запись типа 7 (PCIe ECAM), `PRESENT = 1`, не PIO; окно проходит те же проверки, что MCFG (`EcamWindow::from_entry`, S4-31b) | `_HID` `PNP0A08`, `_CID` `PNP0A03`, `_UID` 0, `_SEG` = сегмент (0), `_BBN` = `start_bus` (0), `_CRS`: `WordBusNumber` (ResourceProducer, MinFixed, MaxFixed) `start_bus..=end_bus` (0..=0), затем `IO(Decode16, 0xCF8, 0xCF8, 1, 8)` (порты conf1 платы S4-30c), `_PRT`: пустой пакет в базовом профиле. В Linux direct boot с PCI NVMe запись типа 8 handoff v1.3 добавляет producer `DWordMemory` для BAR0/1 и INTA → GSI 16 в `_PRT`. [ACPI 6.6 §6.2.14](https://uefi.org/specs/ACPI/6.6/06_Device_Configuration.html#prt-pci-routing-table) |
| `\_SB.ECAM` | та же запись типа 7, `PRESENT = 1` (S4-31b) | `_HID` `PNP0C02` (motherboard resources), `_UID` 0, `_CRS`: `Memory32Fixed(ReadWrite, base, 0x10_0000)` — окно ECAM из записи; выводится после PCI0 и перед COM; нужен для `pci_mmcfg_late_init` (ECAM принимается как зарезервированный ресурс) |
| `\_SB.COM<n>` | запись типа 1 (COM1), `PRESENT = 1`, `PIO` | имя `COM` + цифра `param0` (1–9), `_HID` `PNP0501`, `_UID` = `param0`, `_CRS`: `IO(Decode16, base, base, 1, length)` (`base` и `length` из записи; `base + length ≤ 0x10000`, `1 ≤ length ≤ 255`) и, если `gsi ≠ 0xFFFF_FFFF`, один `Interrupt(ResourceConsumer, …) {gsi}`; режим и полярность из `irq_flags` (бит 0: 1 = level, 0 = edge; бит 1: 1 = active-low), Exclusive |
| `\_SB.RTC0` | опциональная запись RTC/CMOS типа 10 (handoff v1.4), `PRESENT = 1`, `PIO` | `_HID` `PNP0B00`, `_CRS` с `IO(Decode16, 0x70, 0x70, 1, 2)`. FADT использует ту же запись для `CENTURY = 0x32` и признака наличия RTC. [ACPI 6.6, §9.14.1](https://uefi.org/specs/ACPI/6.6/09_ACPI_Defined_Devices_and_Device_Specific_Objects.html#pc-at-compatible-rtc-cmos-devices-pnp0b00) |
| `\_S5_` | `build_dsdt_with_s5` (S4-31e), поле `slp_typ_s5` записи ACPI PM (тип 9) | `Name(\_S5, Package() { 5, 5, 0, 0 })` в корне после `\_SB`: `SLP_TYPa` = `SLP_TYPb` = 5 (`ACPI_SLP_TYP_S5`), два последних элемента нулевые. `build_dsdt` без `_S5` даёт прежние байты (golden S4-31b) |

Для firmware board v1 COM1 даёт `IO 0x3F8 length 8` и `Interrupt(ResourceConsumer, Edge, ActiveHigh, Exclusive) {4}` (IRQ4 по FIRMWARE_BOOT.md §6.3; доставки IRQ4 нет, прошивка опрашивает COM1 — это описание ресурса, а не обещание работы прерывания в гостевой ОС).

У `\_SB.PCI0` нет `_ADR`: у устройства должен быть `_HID` или `_ADR`, не оба (исправлено по ревью S7-12; эталон `both` пересчитан).

**Общие ресурсы PCI0.** Запись ECAM v1.0 передаёт диапазон шин.
В базовой DSDT _CRS содержит этот диапазон и порты conf1 `0xCF8..=0xCFF`.
_PRT возвращает пустой пакет, ACPICA сообщает об этом предупреждением.
В профиле PCI NVMe запись handoff v1.3 передаёт producer MMIO
`0xFC004000..=0xFC007FFF` и фиксированный маршрут INTA на GSI 16.
Это все ресурсы единственного PCI-устройства текущей платы. Формат
апертур и маршрутов дополнительных устройств определяется при их добавлении.
[ISSUE-065](../KNOWN_ISSUES.md#issue-065-s7-dsdt-pci0-has-no-io-mmio-windows-or-interrupt-routes), [ACPI 6.6, §6.2.14](https://uefi.org/specs/ACPI/6.6/06_Device_Configuration.html#prt-pci-routing-table)

### Отложено

| Что | Почему не здесь | Кто делает |
|---|---|---|
| HPET `PNP0103` | Для HPET требуется подтверждённая потребность и модель устройства. | S7-14 |
| TPM `MSFT0101`, `_CRS` CRB `0xFED4_0000` | запись типа 6 `PRESENT = 0` до S7-22; CRB-устройства на плате нет | S7-22 |
| IOAPIC, дисплей, NVRAM в DSDT | описываются таблицами MADT и конфигурацией ОС, а не AML; отдельного решения «описывать ли в DSDT» нет | S7-13 (состав — после S7-30) |
| Публикация RSDP через UEFI configuration table | В текущем blob FADT уже указывает на DSDT полем `X_DSDT`. Путь публикации через UEFI относится к S7-13. | S7-13 |

PCI0, COM1 и RTC0 объявляются по соответствующим записям manifest. Для отложенных устройств потребуются данные manifest и правила вывода в `dsdt.rs`.

### Валидатор

`validate_dsdt` проверяет длину, сигнатуру и нулевую сумму байтов таблицы
по модулю 256. Затем `validate_definition_block` проверяет границы пакетов
в `Scope`, `Device` и данных `Name`, включая вложенные `Package` у `_PRT`,
и уникальность имён. Лимиты глубины пакетов и пространства имён считаются
отдельно. Содержимое Buffer, VarPackage и тела Method рассматриваются
как байты с проверенной длиной. `build_dsdt` вызывает валидатор для своего результата.

### Выборочная сверка с ACPI 6.6

- Сверены расположение битов и диапазон PkgLength, код ReturnOp
  и порядок полей Buffer и Package. Выборочная сверка относится
  к кодированию этих конструкций. [ACPI 6.6, §20.2](https://uefi.org/specs/ACPI/6.6/20_AML_Specification.html)
- Для QWord Address Space Descriptor сверены тег, минимальная длина и
  смещения пяти 64-битных полей. Для диапазона номеров шин Type Specific
  Flags равны нулю по таблице 6.51. В Word и DWord Address Space Descriptor
  бит 0 General Flags записывается нулём, стандарт помечает его как `Ignored`.
  Нулевой checksum End Tag допустим по §6.4.2.9.
  [ACPI 6.6, §6.4](https://uefi.org/specs/ACPI/6.6/06_Device_Configuration.html)
- Для фиксированного INTx-маршрута сверены четыре поля пакета,
  номер функции `0xFFFF` и связь нулевого Source с GSI в Source Index.
  [ACPI 6.6, §6.2.14](https://uefi.org/specs/ACPI/6.6/06_Device_Configuration.html#prt-pci-routing-table)

- Идентификаторы PCI root `PNP0A08`/`PNP0A03` и последовательного порта
  `PNP0501` сверены с документацией Microsoft. RTC `PNP0B00` описан в ACPI 6.6.
  [PCI](https://learn.microsoft.com/en-us/windows-hardware/drivers/pci/accessing-pci-device-configuration-space), [COM](https://learn.microsoft.com/en-us/samples/microsoft/windows-driver-samples/serial-port-driver/), [RTC](https://uefi.org/specs/ACPI/6.6/09_ACPI_Defined_Devices_and_Device_Specific_Objects.html#pc-at-compatible-rtc-cmos-devices-pnp0b00)

Приёмка загрузки Windows и Linux с этими таблицами относится к S7-31/S7-32.

### Проверка

`cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — модули `acpi::aml::tests` (конструкты, отрицательные случаи), `acpi::dsdt::tests` (эталонные DSDT побайтно, вариации manifest, повреждения), `acpi::manifest::tests`, `acpi::table::tests`. Результат — в [отчёте этапа 7](../reports/STAGE_7_REPORT.md#s7-12-aml-кодировщик-и-dsdt-из-manifest).

Повторная сборка обоих профилей, проверка контрольной суммы DSDT и интерпретация _CRS/_PRT через ACPICA описаны в [отчёте S7-12](../reports/STAGE_7_REPORT.md#s7-12-aml-кодировщик-и-dsdt-из-manifest).

## Состав ACPI-таблиц для Linux-пути этапа 4 (S4-31a)

Раздел написан срезом S4-31a [плана этапа 4](../plans/STAGE_4.md#s4-31-подготовить-минимальные-discovery-tables). Он отвечает на вопрос «какие таблицы нужны Linux и почему» и не реализует ни одной из них, кроме MCFG. Раздел «Отложено» выше относится к этапу 7; здесь — состав для data-only blob этапа 4 (direct boot без UEFI, loader — S4-37). Все решения о способе сборки блоба (const fn / статические байты) и о handoff v1.2 приняты в плане S4-31, здесь не повторяются.

### Источники и границы проверки

- Исходники Linux: **linux-source-7.0.0**, частичная распаковка (22 файла) в каталоге сессии `…/scratchpad/xpci/linux-source-7.0.0`: `arch/x86/kernel/acpi/boot.c`, `arch/x86/pci/{mmconfig-shared,mmconfig_64,common,direct,init,acpi}.c`, `drivers/acpi/{osl,bus,reboot}.c`, `drivers/acpi/acpica/{tbfadt,hwacpi,utxfinit,evxfevnt}.c`, `drivers/clocksource/acpi_pm.c`, `drivers/firmware/dmi_scan.c` и ещё несколько. Ссылки ниже — на эти файлы и номера строк именно этой копии.
- **В распаковке нет** и потому **не прочитано**: `include/acpi/actbl*.h` (раскладка структур MCFG, MADT, FADT, FACS, RSDP), `drivers/acpi/tables.c` (`acpi_locate_initial_tables`, `acpi_table_parse`), `drivers/acpi/acpica/tbxfroot.c`/`tbutils.c`/`tbinstal.c` (проверки RSDP, выбор XSDT/RSDT, проверка контрольных сумм при установке), SRAT/SLIT (`drivers/acpi/numa/*`, `arch/x86/mm/*`). Утверждения, которым эти файлы нужны, помечены «не сверено».
- ACPI-спецификация: [раздел выше](#источники-и-редакции) — редакция 6.4 через Context7; формат MCFG определяет PCI Firmware Specification, её текста под рукой не было, раскладка MCFG принята по формату, который читает Linux (`pci_parse_mcfg`), и проверена только эталоном побайтно в тестах (см. «MCFG»).

### Набор и обоснование

| Таблица | Нужна | Почему и откуда видно (Linux 7.0.0) |
|---|---|---|
| **RSDP** | да | Linux берёт корневой указатель в `acpi_os_get_root_pointer` (`drivers/acpi/osl.c:186–218`): параметр `acpi_rsdp=` (только `CONFIG_KEXEC`), затем `acpi_arch_get_root_pointer()`. На x86 это `boot_params.acpi_rsdp_addr` (`arch/x86/kernel/acpi/boot.c:1832–1838`), потом EFI config tables, потом (только `CONFIG_ACPI_LEGACY_TABLES_LOOKUP`) сканирование `acpi_find_root_pointer`. Для direct boot без UEFI адрес RSDP loader кладёт в `boot_params.acpi_rsdp_addr` (S4-37), сканирование области BIOS не нужно. Раскладка и проверка структуры RSDP — **не сверено** (`tbxfroot.c` отсутствует). |
| **XSDT** | да | `acpi=rsdt` выставляет `acpi_gbl_do_not_use_xsdt` (`boot.c`, `parse_acpi`), значит по умолчанию корневой таблицей служит XSDT, и RSDT для работы не нужна; 64-битные адреса нужны для X_-полей FADT. Будет ли Linux принимать RSDP revision 2 с нулевым `RsdtAddress`, **не сверено** (`tbutils.c` отсутствует); проверяется в S4-31g/S4-38. В XSDT перечисляются FADT, MADT, MCFG; DSDT и FACS в корневой таблице **не** перечисляются, их устанавливает ACPICA по полям FADT. |
| **FADT** | да | (1) Через неё устанавливаются DSDT и FACS: `acpi_tb_parse_fadt` (`acpica/tbfadt.c:276–330`) берёт `Xdsdt`, `facs`, `Xfacs`. 64-битный адрес главнее 32-битного (`acpi_tb_select_address`, `tbfadt.c:210–260`). (2) `acpi_parse_fadt` (`boot.c:974–1020`) читает флаги `LEGACY_DEVICES`, `8042`, `NO_CMOS_RTC`, `NO_VGA` и адрес PM-таймера (`xpm_timer_block` при revision ≥ `FADT2_REVISION_ID`, иначе `pm_timer_block`); по нему `drivers/clocksource/acpi_pm.c` создаёт clocksource (`pmtmr_ioport`, строки 37–42, 213–242). (3) `sci_interrupt` — вход `acpi_sci_ioapic_setup` (`boot.c:1229–1233`, если MADT не содержит ISO для SCI) и `acpi_pic_sci_set_trigger` (`bus.c:1401`). (4) `SMI_CMD = 0` допустим: `acpi_hw_set_mode` возвращает «переход невозможен» без ошибки загрузки и `acpi_hw_get_mode` сразу сообщает режим ACPI без чтения `SCI_EN` (`acpica/hwacpi.c:28–70, 120–135`). (5) Reset register: `drivers/acpi/reboot.c:44–60` требует `RESET_REG_SUP`, `RESET_VALUE`. Содержание полей — S4-31e, значения PM-блока — S7-14a. |
| **FACS** | да | `acpi_enable_subsystem` вызывает `acpi_tb_initialize_facs` «для Global Lock и Firmware Waking Vector» и при неудаче возвращает ошибку (`acpica/utxfinit.c:122–133`); `bus.c:1433` вызывает его с флагами `~ACPI_NO_ACPI_ENABLE`, то есть без `ACPI_NO_FACS_INIT`. Без FACS инициализация подсистемы ACPI отказывает. Раскладка — **не сверено**. По плану (решение пользователя 3) FACS лежит в регионе NVS, остальные таблицы — reclaim. |
| **DSDT** | да | Устанавливается из FADT (`tbfadt.c:311–316`), загружается `acpi_load_tables` (`bus.c:1463`). Нужна для `\_SB.PCI0` и для PNP0C02 с `_CRS` на окно ECAM: от этого зависит приём MCFG в поздней фазе (см. «MCFG», пункт 4). Построение — `build_dsdt` (S7-12, раздел выше); PNP0C02 и IO `0xCF8–0xCFF` у PCI0 — S4-31b. |
| **MADT** | да | Без неё x86 Linux не получит ни LAPIC, ни IOAPIC по ACPI: `acpi_parse_madt` читает `madt->address` и флаг `PCAT_COMPAT` (`boot.c:130–155`); `acpi_parse_madt_lapic_entries` без записей LAPIC/x2APIC даёт «No LAPIC entries present», `-ENODEV` (`boot.c:1090`); без записи IOAPIC — «No IOAPIC entries present», `-ENODEV` (`boot.c:1207–1213`). Interrupt Source Override обрабатываются `acpi_parse_int_src_ovr`; для SCI без ISO Linux подставляет собственную настройку (`boot.c:1229–1233`; по плану ISO для SCI на GSI 9, level, active-low задан явно, решение 4). NMI-записи необязательны. Состав записей — S4-31f. |
| **MCFG** | да | Единственный путь к ECAM (расширенное конфигурационное пространство PCIe): `pci_parse_mcfg` (`arch/x86/pci/mmconfig-shared.c:617–660`). Подробности — ниже. |
| **HPET** | нет | `acpi_parse_hpet` (`boot.c:897–`) вызывается как необязательный разбор (`boot.c:1666`) и существует только при `CONFIG_HPET_TIMER`; таблица лишь сообщает адрес и номер блока HPET. Устройства HPET на плате нет (коды 8–15 handoff зарезервированы под S7-14), таблица с несуществующим устройством была бы ложью. Таймеры решаются в S4-29/S7-14. Поведение Linux без HPET в этом ядре **не сверено** из `hpet.c` (нет в распаковке); подтверждается в S4-38. |
| **SRAT, SLIT** | нет | NUMA нет; одна область памяти, один узел. В `boot.c:1577` сказано, что `acpi_table_init()` отдельно позволяет читать SRAT без побочных эффектов, то есть SRAT необязательна; отсутствие узла обрабатывается как `NUMA_NO_NODE` (`arch/x86/pci/acpi.c:457–464`). Код разбора SRAT/SLIT не в распаковке, **не сверено**. |
| **SMBIOS** | не как ACPI | Это не ACPI-таблица; `dmi_scan_machine` при отсутствии SMBIOS пишет «DMI not present or invalid» и продолжает (`drivers/firmware/dmi_scan.c:761`). Последствия отсутствия: `dmi_get_bios_year()` возвращает `-ENODATA` (`dmi_scan.c:1095–1103`), см. ниже. Решение «нужен ли SMBIOS» — S4-37/S4-38, не S4-31. |

Прочие таблицы (BGRT, SPCR, SBF, HEST/GHES, VIOT, PRMT, DMAR, IVRS и т. п.) Linux разбирает как необязательные (`boot.c:1654, 1668, 1673`, `bus.c:1585–1587`); в минимальный набор не входят и не добавляются без отдельной причины.

### MCFG: что Linux проверяет и из чего это следует

Все ссылки — `arch/x86/pci/mmconfig-shared.c` Linux 7.0.0.

1. **Разбор.** `pci_parse_mcfg` (617–658) считает число записей как `(header->length - sizeof(struct acpi_table_mcfg)) / sizeof(struct acpi_mcfg_allocation)`; ноль записей — «MCFG has no entries», `-ENODEV`. Записи — по адресу сразу за структурой `acpi_table_mcfg` (`&mcfg[1]`). Размеры 44 байта (36 + 8 reserved) и 16 байт на запись в `actbl1.h` **не сверены** (файла нет); они взяты из общеизвестного формата и закреплены эталонными байтами в тестах `mcfg.rs`.
2. **Окно выше 4 GiB.** `acpi_mcfg_valid_entry` (599–615): адрес `< 0xFFFF_FFFF` принимается; иначе принимается только OEM ID `SGI`, либо `revision >= 1` и год BIOS из SMBIOS `>= 2010`. SMBIOS у платы нет (`dmi_get_bios_year()` = `-ENODATA`), поэтому окно **должно лежать ниже 4 GiB**; ECAM платы `0xFC10_0000` — подходит. Сериализатор отвергает окно, оканчивающееся выше 4 GiB.
3. **Когда проверяется резервирование окна.** `__pci_mmcfg_init` (`mmconfig-shared.c`, до строки 712) сначала вызывает `pci_mmcfg_reject_broken` → `pci_mmcfg_reserved` (517–) для каждого окна. Ранняя фаза (`pci_mmcfg_early_init`, 715–728, из `pci_arch_init`, `arch/x86/pci/init.c:11–18`, `early = 1`): если год BIOS < 2016 (а `-ENODATA` < 2016) **и** `raw_pci_ops` уже выбран (conf1 прошёл `pci_check_type1`), окно должно быть целиком записью E820 reserved (`is_mmconf_reserved(e820__mapped_all, …)`, 538–546); при `raw_pci_ops == NULL` ранняя проверка просто возвращает `true` (строки 519–549). При успехе `pci_probe` получает `PCI_PROBE_MMCONF` (706), и поздняя фаза уже ничего не разбирает.
4. **Поздняя фаза и PNP0C02.** `pci_mmcfg_late_init` (`bus.c:1584`; `mmconfig-shared.c:730–745`) повторно разбирает MCFG и вызывает `__pci_mmcfg_init(0)` **только если** ECAM ещё не выбран, а другой способ доступа остался разрешённым (`pci_probe & PCI_PROBE_MASK & ~PCI_PROBE_MMCONF`, строка 742), то есть когда ранняя фаза отвергла окно (случай conf1 + E820 без записи). Тогда окно обязано целиком лежать в диапазоне памяти `_CRS` устройства PNP0C01 или PNP0C02 (`is_acpi_reserved` 429–446, `check_mcfg_resource` 379–412); иначе допускается только EFI MMIO, иначе «not using ECAM (… not reserved)». Для окна в одну шину (1 MiB) частичного уменьшения нет (471–515, нижний предел 16 MiB): либо вся, либо отказ. Следствия для этапа 4: (а) на плате, где conf1 отвергается `pci_sanity_check` (S4-30c, `raw_pci_ops == NULL`), ECAM принимается рано без проверок резервирования; (б) если conf1 проходит (появилась функция host bridge или SMBIOS-год ≥ 2001) и SMBIOS-год < 2016 или отсутствует, то ECAM принимается рано только при E820 reserved на `[0xFC10_0000, +1 MiB)`, иначе остаётся PNP0C02 в DSDT (S4-31b) как поздняя страховка; (в) рекомендация: делать оба (PNP0C02 в DSDT — по комментарию Linux `mmconfig-shared.c:535–538` со ссылкой на PCI Firmware r3.3, sec 4.1.2 (сам текст не читался), E820 reserved на окно ECAM добавить в контракт S4-37; в плане E820 для ECAM сейчас нет — открытое). Что именно выберет ядро, зависит от конфигурации и проверяется в S4-38.
5. **Конец шины.** `pci_mmcfg_check_end_bus_number` (325–345) правит `end_bus < start_bus` на 255 и подрезает по соседней записи. Сериализатор ничего не правит: инверсия — ошибка.
6. **Результат.** `pci_mmcfg_arch_init` (`mmconfig_64.c:133–143`) отображает окна и ставит `raw_pci_ext_ops = &pci_mmcfg`. `raw_pci_read` (`common.c:41–50`) использует `raw_pci_ops` только для `domain 0` и `reg < 256` **при наличии** `raw_pci_ops`, иначе всё идёт через `raw_pci_ext_ops`. Поэтому отказ `pci_sanity_check` для conf1 (S4-30b/c, `direct.c:204–216`: нужен DMI-год ≥ 2001, функция класса 0x0600/0x0300 или vendor Intel/Compaq) не оставляет Linux без доступа к PCI, если MCFG принят: вся конфигурация идёт через ECAM. Это объяснение по коду, фактический запуск — S4-38.

### MCFG из записи PCIE_ECAM (`firmware/uefi/src/acpi/mcfg.rs`)

Сериализатор `build_mcfg_from_entry` (и обёртка `build_mcfg` над `BoardManifest`) принимает запись типа 7 handoff block (`param0` — сегмент, `param1` — `start_bus | end_bus << 8`) и пишет 60 байт без `alloc`:

| Смещение | Размер | Поле | Значение |
|---|---|---|---|
| 0 | 4 | Signature | `MCFG` |
| 4 | 4 | Length | 60 |
| 8 | 1 | Revision | 1 |
| 9 | 1 | Checksum | вся таблица в сумме по модулю 256 даёт 0 |
| 10 | 6 | OEM ID | `OPNDSK` (общий с DSDT, `table.rs`) |
| 16 | 8 | OEM Table ID | `ODFWMCFG` |
| 24 | 4 | OEM Revision | 1 |
| 28 | 4 | Creator ID | `ODFW` |
| 32 | 4 | Creator Revision | 1 |
| 36 | 8 | Reserved | 0 |
| 44 | 8 | Base address | `base` записи (плата: `0xFC10_0000`) |
| 52 | 2 | PCI segment | `param0` (плата: 0) |
| 54 | 1 | Start bus | `param1` бит 7..0 (плата: 0) |
| 55 | 1 | End bus | `param1` бит 15..8 (плата: 0) |
| 56 | 4 | Reserved | 0 |

Правила отказа (все до записи байтов; это контракт S4-30c — «ECAM и conf1 на плате», `MACHINE_CONTRACT.md`, дескриптор `PCI_ECAM_DESCRIPTOR`, сегмент 0, bus 0–0, окно 1 MiB):

| Ошибка | Условие |
|---|---|
| `NotEcamEntry` | тип записи не 7 |
| `MissingEcam` | `PRESENT = 0` (на плате без opt-in `pci_ecam` код 7 так и выглядит) |
| `PioWindow` | флаг `PIO` |
| `ReservedParamBits` | `param1` выше бита 15 |
| `SegmentOutOfRange` | `param0` > `0xFFFF` |
| `BusRangeInverted` | `start_bus > end_bus` |
| `LengthMismatch` | `length != (end_bus − start_bus + 1) << 20`, в том числе 0 |
| `ZeroBase`, `BaseUnaligned` | `base == 0` или не кратен 1 MiB (правило проекта: один bus — 1 MiB) |
| `WindowAbove4Gib` | `base + length > 4 GiB` (причина — Linux `acpi_mcfg_valid_entry`, пункт 2) |
| `OutsideStage4Contract` | допустимая запись, но не сегмент 0 и шины 0..=0 (границы этапа 4: один сегмент, одна шина, без host bridge) |
| `Table(TooShort)` | выходной буфер меньше 60 байт; буфер не меняется |

Сериализатор детерминирован: байты зависят только от записи, `OEM Table ID` и `Creator` — собственные строки проекта.

### Что не сделано в этом срезе (S4-31a; позже реализовано в S4-31b…g, см. раздел ниже)

RSDP, XSDT, FADT, FACS, MADT; DSDT с PNP0C02; E820 запись для ECAM; размещение и адреса таблиц; сборка blob; `const fn`-форма сериализатора (решение 1 плана требует статических байт для фиксированных таблиц; текущая функция записывает в буфер вызывающего, как `build_dsdt`, и переводится в `const fn` вместе с `table.rs` на срезе сборки blob).

### Blob v2 и раскладка таблиц (S4-31d…g)

Таблицы собираются `const fn` в `firmware/uefi/src/acpi/blob.rs` (freestanding-бинарь `od-acpi-blob`, секция `.acpi_blob`; `tools/build/build_stage4_acpi_blob.py` только вынимает секцию и читает заголовок). Машинная сторона (`crates/od-machine/src/acpi_blob.rs`) только проверяет blob. Загрузка в RAM и E820 — S4-37.

Регион ACPI платы — `[0xE_0000, 0xF_0000)`: часть для таблиц (E820 ACPI reclaim) `[0xE_0000, 0xE_F000)` и часть NVS `[0xE_F000, 0xF_0000)` для FACS. Blob: заголовок 0x60 байт (magic `ODACPIB1`, `version` 2, `manifest_hash`, `load_base`, `payload_offset`/`payload_len`, `rsdp_address`, `table_count`, `total_len`, `facs_offset`/`facs_len`/`facs_address`), каталог по 16 байт на таблицу (сигнатура, смещение в payload, длина, reserved 0), payload и 64 байта FACS в конце файла.

Раскладка payload (загружается как есть на `load_base` = `0xE_0000`; значения — для эталонного 1104-byte blob `tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin`, в нём 5 таблиц, `payload_len` 0x360):

| Таблица | Адрес (GPA) | Смещение в payload | Длина |
|---|---|---|---|
| RSDP revision 2 | `0xE_0000` | 0x000 (слот 0x40, остаток нулевой) | 36 |
| DSDT (с `\_S5` и `CPU0`) | `0xE_0040` | 0x040 | 273 |
| MADT (`APIC`) | `0xE_0160` | 0x160 | 84 |
| FADT (`FACP`, revision 6) | `0xE_01C0` | 0x1C0 | 276 |
| MCFG (есть при окне ECAM) | `0xE_02E0` | 0x2E0 | 60 |
| XSDT | `0xE_0320` | 0x320 | 60 |
| FACS | `0xE_F000` | вне payload (`facs_offset`, NVS) | 64 |

Порядок таблиц и адреса следуют из кода сборки (16-выравнивание, зазоры нулевые); при другой конфигурации платы (нет ECAM) MCFG отсутствует, и адреса следующих таблиц сдвигаются. XSDT перечисляет FADT, MADT, MCFG (DSDT и FACS в нём нет); RSDP даёт адрес XSDT и нулевой `RsdtAddress`; FADT указывает на DSDT (`DSDT`/`X_DSDT`) и FACS (`FIRMWARE_CTRL`/`X_FIRMWARE_CTRL`, оба `0xE_F000`). FACS не имеет контрольной суммы и не входит в каталог. Открытые вопросы по FADT/FACS и упаковщику — [ISSUE-497…500](../KNOWN_ISSUES.md).


## Публикация таблиц UEFI (S7-13)

Прошивка проверяет handoff и строит таблицы ACPI при наличии записи fixed hardware и согласованных с ней диапазонов. RSDP ревизии 2 установлен под `EFI_ACPI_20_TABLE_GUID` и указывает на XSDT. XSDT содержит FADT и MADT, а при наличии ECAM также MCFG. FADT ссылается на DSDT и FACS. Прошивка размещает FACS в `EfiACPIMemoryNVS` по GPA `0xEF000`, остальные байты в `EfiACPIReclaimMemory` начиная с `0xE0000`. Установщик публикует RSDP после заполнения обоих диапазонов.

FADT использует Major Version 6 и байт Minor Version `0x05`, буквальное значение строки Table 5.9 ACPI 6.6. Общее пояснение о синхронности версий таблицы и документа подразумевало бы `0x06`. В истории редакций ACPI 6.6 отдельного изменения этого поля нет. `WBINVD=1` отражает поддержку инструкции в последовательном интерпретаторе с когерентной памятью и без моделируемых dirty cache lines. `X_FIRMWARE_CTRL` и `FIRMWARE_CTRL` указывают на один FACS, OSPM отдаёт приоритет доступному 64-битному адресу. [ACPI 6.6 §5.2.9](https://uefi.org/specs/ACPI/6.6/05_ACPI_Software_Programming_Model.html#fixed-acpi-description-table-fadt), [история редакций ACPI 6.6](https://uefi.org/specs/ACPI/6.6/Frontmatter/Revision_History.html).

MADT описывает один LAPIC и один IOAPIC из manifest, маршрут SCI с параметрами PM-блока и закреплённое для платы отображение IRQ0 на GSI2. `PCAT_COMPAT=0`. DSDT берёт устройства из тех же записей, а MCFG описывает только переданное окно ECAM. HPET и TPM2 требуют подключённых устройств. Для TPM2 точка расширения находится при формировании набора таблиц, реализация принадлежит S7-22.

`SMBIOS3_TABLE_GUID` указывает на физический 64-битный entry point `_SM3_`. Структуры хранятся рядом в выделенной странице `EfiACPIReclaimMemory`. UEFI-указатель и адрес внутри entry point остаются физическими при переходе в виртуальный режим runtime. [UEFI 2.11 §4.6](https://uefi.org/specs/UEFI/2.11/04_EFI_System_Table.html), [DSP0134 3.9 §§5.2.2, 6.2, 7](https://www.dmtf.org/sites/default/files/standards/documents/DSP0134_3.9.0.pdf).

| Тип | Источник данных |
|---|---|
| 0 | Название, дата и версия прошивки Open Desktop, размер ROM, признаки UEFI и виртуальной машины, ACPI по записи PM-блока. TPM описывается Type 43 при подключении устройства. |
| 1 | UUID в формате SMBIOS byte order и серийный номер дословно из handoff. Для полной проверки обязательных полей профиль передаёт ненулевой UUID. |
| 2, 3 | Виртуальные плата и корпус, собственные строки и ссылка на handle корпуса. |
| 4 | Processor ID из `CPUID(1).EAX/EDX` профиля DiagnosticV1, один виртуальный CPU из handoff. Max Speed и Current Speed из `virtual_cpu_mhz` v1.9. В ранних версиях handoff скорость обозначена как неизвестная. |
| 16, 17 | Один массив и одно устройство виртуальной RAM. Ёмкость включает вырезанные из RAM загрузочные и ACPI reservation. Большие объёмы используют Extended Size. |
| 19, 20 | Одна пара записей на непрерывный банк RAM и ссылки на массив и устройство. Разрыв между low RAM и RAM выше 4 GiB сохраняется. |
| 32, 127 | Состояние загрузки и конец таблицы. |

Type 7 опущен по DSP0134 Table 4, так как внешние кэши не моделируются. Ссылки на кэши Type 4 равны `FFFF`. Type 9 опущен, так как у платы нет обновляемых слотов. Тип 20 включён для сценария этапа 7 независимо от его опциональности в DSP0134.

Для сравнения firmware и data-only результата используются один профиль и один запечатанный handoff.

```sh
od-runner dump-handoff-block --fv-app acpi --out work/stage7/acpi-handoff.bin
python3 tools/build/build_firmware.py --with-test-apps --out work/stage7/firmware-tests.bin
od-runner boot-firmware --image work/stage7/firmware-tests.bin --fv-app acpi \
  --budget 15000000 --com1-out work/stage7/acpi.com1 --acpi-out work/stage7/acpi-guest.bin
python3 tools/build/build_firmware.py --acpi-blob work/stage7/acpi-handoff.bin \
  --compare-acpi-guest work/stage7/acpi-guest.bin --out work/stage7/acpi-blob.bin
```

Ожидаемый код runner для возврата тестового приложения и `STOP 0x21` равен 6. Программа S7-13 читает установленные configuration tables, проверяет указатели и контрольные суммы и исполняет `WBINVD`. Режим `--acpi-blob` вычисляет байты в сборке `x86_64-unknown-none`, а Python извлекает ELF-секцию и проверяет раскладку, контрольные суммы и ссылки. Сравнение охватывает все 64 KiB reclaim и NVS, включая неиспользованное место.
