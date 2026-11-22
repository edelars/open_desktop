# Профиль виртуальной платы v0.1 — выбранная проектная конфигурация

**Дата:** 22 октября 2026 года. **Статус:** проектная конфигурация S0-05 принята для следующих этапов; статические драйверы выбранных Windows 26H2 образов частично проверены, устройства и boot path Windows не запускались (S7-32 пропущена по решению владельца 2026-11-21). Это план целевой платформы, а не обещание совместимости.

Профиль ориентирован на Windows 11 Pro x64 26H2 и раннюю загрузку Linux x86-64. Ранний CPU/JIT fragment runner из [CPU contract](CPU_CONTRACT.md) не использует эту плату и не считается доказательством загрузки гостя.

## Выбранные границы

Целевой SMP-профиль и ограничения CPUID/MADT описаны в [SMP_CONTRACT.md](SMP_CONTRACT.md); SMP не входит в текущую исполняемую конфигурацию платы.

| Подсистема | Проект v0.1 | Основание и статус проверки |
|---|---|---|
| Загрузочный путь | Прямая загрузка Linux через Linux/x86 Boot Protocol для отладки; Windows — только через собственную UEFI. Legacy BIOS/CSM не входит в профиль. | Разделяет быстрый Linux CPU bring-up и обязательный UEFI Windows path. Linux kernel/initramfs и Windows image ещё не прогонялись. Контракт handoff, ROM/NVRAM windows и boot reservations описаны в [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) и приняты в S7-02; host-side ROM backing, image loader, handoff и board profile реализованы и приняты в S7-03. **Статус на 2026-11-21 (S7-41):** собственный UEFI boot path (ядро, Boot Services, PE, NVMe/FAT, boot manager, TPM, Secure Boot) реализован и принят по задачам ступеней 1–6 (G1–G3 — S7-29); собственные тестовые EFI-приложения запускаются из FV и с ESP. Linux kernel через собственную UEFI доходит до `/init` на диагностической единой плате (S7-31m), приёмка S7-31 не выполнена. Windows image через собственную UEFI не запускался (S7-32 пропущена по решению владельца 2026-11-21); совместимость с Windows не заявляется. [Отчёт этапа 7](../reports/STAGE_7_REPORT.md#0-итог-этапа-и-приёмка-ступеней-16-s7-41-2026-11-21). |
| CPU / vCPU | Windows board profile требует не менее 2 vCPU по опубликованной Microsoft reference-конфигурации VM; начальный S0-09 runner остаётся одновекторным и отдельно ограниченным. Реализация нескольких vCPU описана в [плане этапа 6](../plans/STAGE_6.md), но ещё не принята. | Не следует путать прототип с board CPU. Точный CPUID/MSR/ISA профиль не выбран; см. [CPU contract](CPU_CONTRACT.md). |
| Гостевая RAM | Начальный boot target — 4 GiB как публичный минимум Windows 11; приложение и рабочий объём памяти уточнить на выбранных сценариях. | 4 GiB — минимум платформы, а не обещание комфортной работы. Диапазоны RAM и MMIO должны быть непересекающимися. |
| Единоличный владелец физической карты гостя | `od-machine` владеет guest physical map и маршрутизацией RAM/MMIO. UEFI memory map, ACPI и Linux `boot_params`/E820 строятся как согласованные представления одной карты. | Не допускать отдельных несогласованных карт в firmware, CPU и устройствах. Реализация не начата. |
| Firmware | Собственная x86-64 UEFI; UEFI 2.11. Постоянные UEFI variables хранятся отдельно от диска гостя. Secure Boot capable как целевое свойство; включение, ключи и проверка подписи — отдельные приёмочные условия. | Windows target не использует сторонний firmware runtime. ISO и EFI-обращения ещё не исследованы. Секретные ключи не хранить в Git. План реализации — [этап 7](../plans/STAGE_7.md). Контракт входа — [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md), принят в S7-02; host-side ROM/image/handoff path принят в S7-03. Собственный UEFI boot path реализован (S7-04…S7-19), запуск гостевых ОС не подтверждён: статус — в строке «Загрузочный путь». [UEFI_PROFILE.md](UEFI_PROFILE.md) — поддержанный профиль (43 слота Boot Services, 14 слотов Runtime Services, четыре из них возвращают `EFI_UNSUPPORTED`); Secure Boot (S7-24, S7-25) принят на собственных образах и ключах; [VTPM.md](VTPM.md) — границы состояния (S7-20 принят после архитектурного ревью 2026-11-18) и команды фазы A (S7-21 phase-A scope принят после независимого ревью 2026-11-18; не означает TPM/PTP compliance; CRB и measured boot — S7-22). |
| Платформенные таблицы | ACPI 6.6; начать с RSDP/XSDT/FADT/MADT/MCFG и минимальных описаний устройств по фактическим обращениям Windows/Linux. SMBIOS 3.9 — инвентаризационная таблица после проверки, что её требует выбранный путь. | Использовать один источник данных платы; не заполнять таблицы фиктивными устройствами. Набор таблиц предварительный и требует trace выбранного media. Таблицы по плану строятся из handoff block (контракт — S7-02, состав — S7-13; [план этапа 7](../plans/STAGE_7.md); контракт — [ACPI_SMBIOS_TABLES.md](ACPI_SMBIOS_TABLES.md); S7-12 и S7-13 приняты). **Статус на 2026-11-21:** таблицы публикуются из handoff block (RSDP v2, XSDT, FADT, FACS, MADT с одним LAPIC, MCFG, DSDT, TPM2 при TPM, SMBIOS); HPET и метод `_OSC` отсутствуют ([ISSUE-642](../KNOWN_ISSUES.md#issue-642), не дефект, отложено); Linux разбирает таблицы без ошибок разбора (S7-31n), Windows таблицы не читал. |
| Прерывания и таймер | Начать с одинарного Local APIC + IOAPIC и явной таблицы маршрутизации; x2APIC и hot-plug отложить. SMP описан в [плане этапа 6](../plans/STAGE_6.md), но ещё не реализован. Выбрать один виртуальный источник системного времени после CPU profile и boot trace. | Типовой x86-64 путь, но точные MSR/interrupt/timer requirements Windows build ещё не сняты. Скорость не использовалась как аргумент. |
| PCIe | Один root complex, один сегмент и статическая топология; PCIe Base 7.1 программный интерфейс, ACPI MCFG для ECAM. Сначала только необходимые BAR, конфигурация и MSI/MSI-X/INTx варианты, реально нужные выбранным устройствам. | Не реализовывать всю PCIe спецификацию. Набор функций и адресные окна определить после выбора device models. |
| Диагностика | Целевой профиль — один 16550A-compatible UART как COM1 (`I/O 0x3F8`, IRQ4); VMHost должен сохранять байты serial output с временными метками. Отдельный собственный wire format для вывода гостевой ОС не вводить. | Это проектная цель. Bounded polling TX slice описан ниже; полный UART, baud rate, IRQ behavior и доступность для выбранных Windows/Linux образов не подтверждены. |
| Канал агента | Второй 16550-совместимый UART как COM2 (`I/O 0x2F8–0x2FF`, IRQ3), профиль 16450 без FIFO; байтовый поток для протокола гостевого агента ([AGENT_PROTOCOL_V1.md](../../protocols/guest/AGENT_PROTOCOL_V1.md)), с ограниченными host-очередями RX/TX и счётчиками потерь. Включается опцией платы `agent_channel`; без неё `0x2F8` не занят (ловушка или absent-UART Linux-профиля). | **Предложенная правка (S11-21), ожидает подтверждения владельца** (вопрос в [ADR-0007](../decisions/ADR-0007-agent-channel-transport.md)). Устройство и host ABI: [AGENT_CHANNEL_DEVICE.md](AGENT_CHANNEL_DEVICE.md), [AGENT_HOST_ABI_V1.md](AGENT_HOST_ABI_V1.md). IRQ3 маршрутизируется в IOAPIC INTIN3, канал опрашивается в циклах платы `firmware_board`; запись COM2 в манифесте платы (handoff, DSDT) не сделана (ISSUE-659): без неё гость находит порт только по известному адресу. |
| Boot storage | RAW disk backend в `od-storage`, для гостя — собственный минимальный NVMe controller и PCIe transport. | NVMe 2.4 Base, NVM Command Set 1.3, NVMe over PCIe Transport 1.4. В Windows Pro WIM, индекс 6, `stornvme.inf` сопоставляет PCI class ID `PCI\CC_010802`, а service `stornvme` имеет boot-start. Доступ прошивки к EFI-разделу требует Block I/O (S7-17). **Статус S7-16 — принят минимальный профиль.** Один NS, LBA 512 байт, admin и I/O с постоянным RAW backend в `od-host`. Профиль firmware опрашивает MMIO `0xFC00_1000`. Opt-in Linux direct boot публикует PCI `00:04.0`, BAR0/1 `[0xFC00_4000, 0xFC00_8000)`, INTx через IOAPIC и пять векторов MSI-X. Linux подтвердил чтение RAW и MSI-X для Admin и I/O. Полный PCIe Transport 1.4 требует FLR, этот разрыв отслеживается отдельно. [NVME_DEVICE.md](NVME_DEVICE.md) · [ISSUE-591](../KNOWN_ISSUES.md#issue-591). |
| Network | Для первого подключённого NIC — модель, совместимая с Intel 82540EM, PCI ID `8086:100E`; host NAT/backend остаётся в host-компоненте. | Выбор подтверждён владельцем. В Windows Pro WIM, индекс 6, есть `nete1g3e.inf` с этим hardware ID. `netkvm.inf` в образе не найден, поэтому VirtIO потребует собственного Windows-драйвера. Device emulation, Linux guest driver, host backend и сетевой сценарий не реализованы и не проверены; выбор сделан по наличию Windows inbox-драйвера. Сетевое устройство реализует S11-32 (этап 11); сетевые протоколы UEFI (SNP, PXE, HTTP boot) не входят в профиль прошивки v1 (решение владельца 2026-11-07, [план этапа 7](../plans/STAGE_7.md#11-решения-владельца-проекта-2026-11-07), D1). |
| Экран | UEFI GOP framebuffer как firmware/setup/diagnostic output; собственный WDDM 2.0/GPU stack не входит в v0.1 и не подменяется GOP. Собственное дисплейное устройство с framebuffer по контракту [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md) (этап 10): версия 2 с курсором и очередью событий и конфигурация с двумя экранами (второй регион `0xF400_0000`, блок `0x300–0x3FC`, `CAPS` бит 3) реализованы в `od-machine`; ABI профилем 4 (тестовый), транспорт v3 и два окна в просмотрщике есть. Проверка на probe гостя (S10-08) и на железе не выполнена. Первый экран — `0xF800_0000`. | Windows 11 заявляет DirectX 12 + WDDM 2.0 в общих требованиях; GOP не доказывает это требование. Сценарий Blender на этапе 0 — только CPU/headless, если выбранная версия это подтвердит. |
| Ввод | Один стандартный PCI xHCI controller (class `0C0330`, xHCI (интерфейс 1.0, HCIVERSION 0x0100; сверка по тексту 1.2)) с USB HID keyboard и mouse; дополнительных USB функций в v0.1 нет. Собственная UEFI подключает USB keyboard к `EFI_SIMPLE_TEXT_INPUT_PROTOCOL`; Windows Setup и установленная система используют inbox xHCI/HID stack. | В Windows Pro `install.wim` (index 6) и обоих образах `boot.wim` найдены `usbxhci.inf`, `usbhub3.inf` и `input.inf`. INF связывают xHCI с class ID `PCI\CC_0C0330`, а generic HID с USB class `03`; это подтверждает наличие candidate drivers, но не их запуск или работу ввода. Для UEFI нужен собственный pre-boot USB/HID stack и передача состояния устройства при выходе из Boot Services. USB-клавиатура для `ConIn` — S7-34 ([план этапа 7](../plans/STAGE_7.md)), blocked до xHCI-модели платы; до неё `ConIn` — COM1 (S7-07). Модель xHCI и USB HID — этап 8 (S8-41…S8-43, решение по рекомендации Opus high 2026-11-21, подтверждено владельцем проекта 2026-11-21); контракт v1 — [XHCI_DEVICE.md](XHCI_DEVICE.md) (S8-41, S8-41 принят 2026-11-21): PCI `00:05.0`, BAR0 `0xFC00_8000` (16 KiB), два порта USB 2.0, клавиатура (boot protocol) и **абсолютный указатель** вместо «mouse», опрос и INTx без MSI/MSI-X. |
| TPM / Secure Boot state | Целевой security device — собственная vTPM 2.0; device state и UEFI variables принадлежат persistent state VM и входят в согласованные snapshots. | Требования Windows задают TPM 2.0. Сверяться с TPM Library v185 и PC Client PTP 1.07/Errata 1.1; ни реализация, ни соответствие/сертификация не заявлены. Размещение (crate `od-tpm`, CRB-устройство в `od-machine`), криптобиблиотеки, профиль команд, интерфейс CRB в окне `0xFED4_0000` (только locality 0) и границы состояния — [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) (S7-20, принят после архитектурного ревью 2026-11-18). [VTPM.md](VTPM.md) содержит «Состояние и границы» (S7-20) и команды фазы A, PCR и формат snapshot `ODTPMSTA` v1 (S7-21, phase-A scope принят после независимого ревью 2026-11-18; TPM/PTP compliance не заявляется, ограничения — [ISSUE-510](../KNOWN_ISSUES.md#issue-510-s7-21-phase-a-acceptance-scope)); CRB, measured boot, фаза B и «Persistent state v1» добавляют S7-22 и S7-23 ([план этапа 7](../plans/STAGE_7.md)), приняты (S7-23 — с отклонениями, [ISSUE-599](../KNOWN_ISSUES.md#issue-599) закрыт; Windows-пункты — [ISSUE-598](../KNOWN_ISSUES.md#issue-598), blocked до S7-32 и этапа 8); контракт — [VTPM.md](VTPM.md); соответствие TPM/PTP не заявляется. Постоянное защищённое состояние ключей — S9-27 (этап 9). |
| Не включено | Audio, USB passthrough, GPU acceleration, nested virtualization, IOMMU, SMP, hot-plug и миграция. | SMP остаётся вне текущей реализации; границы и задачи приведены в [плане этапа 6](../plans/STAGE_6.md). Остальное добавлять по отдельным требованиям и этапам, а не ради полноты виртуальной платы. Audio: класс выбран архитектурным ревью 2026-11-22 (HDA-совместимый), ожидает подтверждения владельца — [AUDIO_DEVICE.md](AUDIO_DEVICE.md). |

Установочный диск подготавливается на host как raw-образ GPT/FAT32 и
подключается к существующему NVMe с секторами 512 байт. Тест совместимости
использует собственное EFI-приложение. Официальный Windows ISO конвертируется
автоматически с разбиением `install.wim` в SWM. Загрузка Windows до Setup
относится к S7-32. [S7-35](../plans/STAGE_7.md#s7-35-транспорт-установочного-носителя-windows).
[ADR-0012](../decisions/ADR-0012-install-media-transport.md).

## Текущий COM1 polling slice

Локальная `Uart16550PollingTx` привязывается к PIO диапазону COM1
`0x3F8–0x3FF`. Она реализует ограниченный byte-only TX профиль с явным
host-controlled переносом holding → shift и завершением передачи. Это не
полная модель 16550A и не меняет целевую карту платы.

Ограниченный CPU path для bare `IN AL,DX` и `OUT DX,AL` в Long64/CPL0 теперь
подключён к COM1 через bounded paged probe (`S4-24b/c`). Это не общий I/O
privilege matrix: IOPL/TSS, остальные widths/forms, IRQ4 и маршрутизация через
APIC/IOAPIC остаются открытыми. Поэтому целевой Linux serial console и полная
16550A совместимость не подтверждены.

Для S4-25a добавлен отдельный one-CPU fixed-edge Local APIC device по xAPIC
MMIO base `0xFEE00000` с APIC ID 0. Это самостоятельный `MachineBus` device
slice, без default board manifest/wiring, version/LVT, IOAPIC routes, timer и
CPU interrupt delivery. Он не меняет описание Linux discovery или объявленный
APIC-профиль гостю.

S4-25b adds an explicitly selected CPU profile for bare `RDMSR`/`WRMSR`
`IA32_APIC_BASE` at the fixed base. The machine helper applies enable/disable
effects synchronously to the standalone S4-25a Local APIC. The profile is not
part of the general paged vCPU session or default board wiring; relocation,
x2APIC, interrupt delivery and Linux APIC discovery remain unimplemented.

S4-25c lets guest software issue one fixed self-IPI through xAPIC ICR. The
Local APIC records it in IRR and the existing explicit acknowledge/EOI model
can move it through device state. This remains a standalone device path: no
CPU handler execution, default board registration, IOAPIC route or Linux APIC
discovery is established.

S4-25d adds an opt-in exact-encoding Long64/CPL0 CR8 profile connected to the
standalone Local APIC TPR. It demonstrates self-IPI priority gating inside the
device model, without enabling CPU interrupt acceptance or adding the profile
to the default board, paged session, or JIT.

S4-26a adds an opt-in standalone IOAPIC profile at `0xFEC00000` with one
INTIN4 fixed-edge route to Local APIC ID 0. It is not registered in the default
board and does not establish device-to-pin mapping, CPU interrupt handling or
guest OS discovery.

S4-26b adds an optional active-high Fixed level mode for that same route, with
Local APIC TMR and IOAPIC Remote IRR tracking through matching EOI and atomic
retrigger. The two models share one narrow state owner, while independent reset
preserves the other controller and an explicit pair reset clears both. This
remains an opt-in device test profile, not default board registration or CPU
handler delivery.

S4-27a adds a caller-driven diagnostic bridge that takes one eligible request
from this APIC profile through a Long64/CPL0 interrupt gate and attempts at
most one handler instruction using flat identity RAM. A handler fault does not
undo successful entry or APIC acknowledgement. It does not register APIC
devices on the default board or provide automatic IRQ scheduling, paged
delivery, guest EOI/IRET, HLT wakeup or Linux discovery.

S4-27b's bounded fixture uses a guest Dword write to the Local APIC EOI
register, executes same-CPL0 IRETQ, and resumes one interrupted instruction.
An asserted level route can re-pend at EOI; delivery remains caller-driven.
This does not add default board registration, paged IRETQ, HLT wakeup or Linux
interrupt support.

## Карта адресов firmware board v1 (S7-02)

Контракт [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) принят в S7-02; host-side ROM backing, handoff и профиль платы реализованы и приняты в S7-03. Полная таблица, boot reservations, handoff block и проверки пересечений описаны в разделах 2 и 15 контракта. Собственный firmware boot path реализован (см. строку «Загрузочный путь»); запуск Windows не подтверждён, Linux — только до `/init` на диагностической плате. Ширина физического адреса 36.

| Диапазон | Назначение |
|---|---|
| `[0x0000_0000, min(R, 0xF400_0000))` | Нижняя RAM; внутри неё handoff block `0x0010_0000` (64 KiB), временный стек, shadow-копия прошивки `[0x0020_0000, 0x0060_0000)`, временные таблицы страниц с `0x0060_0000` |
| `[0xF400_0000, 0xFC00_1000)` | Дисплей (`DISPLAY_DEVICE.md` §3, §16.3) |
| `0xFEC0_0000`, `0xFED4_0000`, `0xFEE0_0000` | IOAPIC, TPM CRB, Local APIC |
| `[0xFF00_0000, 0xFF04_1000)` | Окно NVRAM: store 256 KiB + страница регистров (устройство — S7-10) |
| `[0xFFC0_0000, 0x1_0000_0000)` | ROM прошивки 4 MiB, read-only; исполняется shadow-копия в RAM, не ROM |
| `[0x1_0000_0000, …)` | Верхняя RAM, `R − 0xF400_0000`, не выше `2^36` |

Общий объём RAM `R`: от 128 MiB до `0xF_F400_0000`, кратно 2 MiB. Окно ECAM `[0xFC10_0000, 0xFC20_0000)` выбрано в S4-30a для bus 0 и входит в карту платы только под opt-in флагом `FirmwareBoardConfig::pci_ecam` (S4-30c, по умолчанию выключен): с флагом это не-RAM MMIO-регион, PCI-config устройство (ECAM + conf1 `0xCF8..=0xCFF`) на шине платы, код 7 handoff — `PRESENT = 1`; bus 0 пустой (host bridge и функций плата не объявляет). Без флага окна нет, код 7 — `PRESENT = 0`, handoff v1.0 прежний байт в байт. ACPI fixed hardware (PM1a evt/cnt, PM timer, reset register) — блок портов `0x0600..0x0610` (S7-14a; `PM1a_EVT_BLK = 0x0600`, `PM1a_CNT_BLK = 0x0604`, `PM_TMR_BLK = 0x0608`, `RESET_REG = 0x060C`, `SMI_CMD = 0`); блок не пересекает COM1 `0x3F8..=0x3FF` и conf1 `0xCF8..=0xCFF`, `0xCF9` остаётся `UnknownOffset`; в карту памяти он не входит; регистрируется под opt-in флагом `FirmwareBoardConfig::acpi_pm` (по умолчанию выключен). В handoff его описывает код 9 (S4-31c), но только при втором opt-in флаге `FirmwareBoardConfig::acpi_tables` (по умолчанию выключен, требует `acpi_pm`): `PRESENT | PIO`, порты блока, SCI — GSI 9, level, active-low (через ISO; запись MADT — S4-31f); `acpi_pm` один handoff не меняет. Область ACPI-таблиц `[0x000E_0000, 0x000F_0000)` (64 KiB) — часть нижней RAM, а не отдельный регион карты: единая карта платы не меняется, при `acpi_tables` таблица регионов handoff режет нижнюю RAM на `RAM` `[0, 0xE_0000)`, `ACPI_TABLES` `[0xE_0000, 0xE_F000)` (E820 reclaim в S4-37; RSDP на `0xE_0000`), `ACPI_NVS` `[0xE_F000, 0xF_0000)` (E820 NVS в S4-37; FACS на `0xE_F000`) и нулевую `RAM` `[0xF_0000, 0x10_0000)`; область не пересекается с handoff (`0x10_0000`), ROM, ECAM, NVMe, IOAPIC, LAPIC, дисплеем и framebuffer. Окно HPET не назначено (S7-14). RTC/CMOS `0x70/0x71` — опциональны: при `rtc_base_unix_seconds` в handoff v1.4 привязывается один RTC по shared virtual clock; без него сохраняется absent-поведение Linux-профиля.

## Владение состоянием и адресами

- `od-machine` назначает каждую область guest physical RAM, ROM/firmware и MMIO ровно одному владельцу; перекрывающиеся назначения отвергаются.
- PCI configuration space и BAR принадлежат модели PCI/device в machine layer; host backend исполняет файловый/сетевой ввод-вывод через узкий интерфейс.
- UEFI variable store, vTPM state и virtual disk — независимые persistent artifacts с разными правилами snapshot/restore. Они не смешиваются с обычной RAM.
- Все адреса и размеры задаются с проверкой переполнения до выделения или передачи в device backend.
- Таблицы ACPI/SMBIOS генерируются из принятого manifest платы; они не ведут второй независимый реестр устройств.

Размещение следует [карте проекта](../PROJECT_LAYOUT.md): CPU/MMU/device models — `crates/od-machine`; firmware — `firmware/uefi`; storage — `crates/od-storage`; host execution/backend — `crates/od-host`; guest drivers — `guest/`; wire formats — `protocols/guest/`.

## Условия до принятия профиля

1. Edition/build локального ISO подтверждены по `sources/install.wim` (Pro, индекс 6, build 26300.9457); UEFI boot files присутствуют. WinPE/Setup в `boot.wim` имеют build 26100.9457; установить происхождение ISO и проверить boot path/совместимость installer с установленной сборкой.
2. Статически проверены WIM driver packages для NVMe и Intel 82540EM-compatible NIC: INF, доступные hardware ID, а для NVMe — boot-start service. Завершить mapping выбранных устройств, включая input, проверить UEFI Block I/O и подтвердить фактический boot/load путь на работающей плате.
3. Зафиксировать CPU profile: CPUID leaves/bits, MSRs, mode transitions, APIC/timer и guest exception contract. Проверять по Intel/AMD manuals и конкретному Windows/Linux boot path.
4. Сверить UEFI protocols, ACPI tables, PCI IDs/BAR/interrupts, NVMe subset, TPM commands/interface и Secure Boot states с фактическими запросами bootloader/OS.
5. Подготовить один функциональный Linux boot сценарий и один Windows installer/boot сценарий; сохранять конфигурацию, hash media и полный диагностический лог.
6. Скоростные решения по device model и критичным CPU путям принимать только после измерений по [протоколу](../testing/BENCHMARKS.md).
7. Проверить serial diagnostics и полный keyboard/mouse path: собственная UEFI ConsoleIn, Windows Setup из `boot.wim`, установленная Windows из `install.wim` и выбранный Linux kernel. Статическое наличие INF не закрывает эту проверку.

## Нормативные источники и редакции

- UEFI Forum, [UEFI Specification 2.11](https://uefi.org/specs/UEFI/2.11/), декабрь 2024.
- UEFI Forum, [UEFI 2.11 console support](https://uefi.org/specs/UEFI/2.11/12_Protocols_Console_Support.html) и [USB support](https://uefi.org/specs/UEFI/2.11/17_Protocols_USB_Support.html): ConsoleIn и USB host/bus driver protocols.
- UEFI Forum, [ACPI Specification 6.6](https://uefi.org/specs/ACPI/6.6/), май 2025.
- DMTF, [SMBIOS Reference Specification 3.9](https://www.dmtf.org/standards/smbios), август 2025.
- PCI-SIG, [PCI Express Base Specification 7.1](https://pcisig.com/specification-overview/pci-express-base), опубликована 17 сентября 2026.
- NVM Express, [specification set](https://nvmexpress.org/specifications/): Base 2.4, NVM Command Set 1.3, PCIe Transport 1.4, ratified 31 июля 2026.
- OASIS, [Virtual I/O Device (VIRTIO) 1.3](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html), Committee Specification 01.
- Trusted Computing Group, [TPM 2.0 Library](https://www.trustedcomputinggroup.org/resources/documents/tpm-library-specification) version 185 и [PC Client Platform TPM Profile](https://www.trustedcomputinggroup.org/resources/documents/pc-client-platform-tpm-profile-ptp-specification) version 1.07 с Errata 1.1.
- Intel, [eXtensible Host Controller Interface (xHCI) Revision 1.2](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf); USB-IF, [HID Device Class Definition 1.11](https://www.usb.org/document-library/device-class-definition-hid-111).
- Microsoft, [HID transport overview](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/hid-transports), [HID over USB](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/hid-over-usb) and [USB host-side drivers](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/usb-3-0-driver-stack-architecture): inbox HID and xHCI driver behavior; the selected ISO is checked separately above.
- Microsoft Windows 11 hardware and VM requirements: см. ссылки и оговорки в [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md).
