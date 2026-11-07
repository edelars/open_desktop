# Профиль виртуальной платы v0.1 — выбранная проектная конфигурация

**Дата:** 22 октября 2026 года. **Статус:** проектная конфигурация S0-05 принята для следующих этапов; статические драйверы выбранных Windows 26H2 образов частично проверены, устройства и boot path не запускались. Это план целевой платформы, а не обещание совместимости.

Профиль ориентирован на Windows 11 Pro x64 26H2 и раннюю загрузку Linux x86-64. Ранний CPU/JIT fragment runner из [CPU contract](CPU_CONTRACT.md) не использует эту плату и не считается доказательством загрузки гостя.

## Выбранные границы

| Подсистема | Проект v0.1 | Основание и статус проверки |
|---|---|---|
| Загрузочный путь | Прямая загрузка Linux через Linux/x86 Boot Protocol для отладки; Windows — только через собственную UEFI. Legacy BIOS/CSM не входит в профиль. | Разделяет быстрый Linux CPU bring-up и обязательный UEFI Windows path. Linux kernel/initramfs и Windows image ещё не прогонялись. |
| CPU / vCPU | Windows board profile требует не менее 2 vCPU по опубликованной Microsoft reference-конфигурации VM; начальный S0-09 runner остаётся одновекторным и отдельно ограниченным. | Не следует путать прототип с board CPU. Точный CPUID/MSR/ISA профиль не выбран; см. [CPU contract](CPU_CONTRACT.md). |
| Гостевая RAM | Начальный boot target — 4 GiB как публичный минимум Windows 11; приложение и рабочий объём памяти уточнить на выбранных сценариях. | 4 GiB — минимум платформы, а не обещание комфортной работы. Диапазоны RAM и MMIO должны быть непересекающимися. |
| Единоличный владелец физической карты гостя | `od-machine` владеет guest physical map и маршрутизацией RAM/MMIO. UEFI memory map, ACPI и Linux `boot_params`/E820 строятся как согласованные представления одной карты. | Не допускать отдельных несогласованных карт в firmware, CPU и устройствах. Реализация не начата. |
| Firmware | Собственная x86-64 UEFI; UEFI 2.11. Постоянные UEFI variables хранятся отдельно от диска гостя. Secure Boot capable как целевое свойство; включение, ключи и проверка подписи — отдельные приёмочные условия. | Windows target не использует сторонний firmware runtime. ISO и EFI-обращения ещё не исследованы. Секретные ключи не хранить в Git. |
| Платформенные таблицы | ACPI 6.6; начать с RSDP/XSDT/FADT/MADT/MCFG и минимальных описаний устройств по фактическим обращениям Windows/Linux. SMBIOS 3.9 — инвентаризационная таблица после проверки, что её требует выбранный путь. | Использовать один источник данных платы; не заполнять таблицы фиктивными устройствами. Набор таблиц предварительный и требует trace выбранного media. |
| Прерывания и таймер | Начать с одинарного Local APIC + IOAPIC и явной таблицы маршрутизации; x2APIC, SMP и hot-plug отложить. Выбрать один виртуальный источник системного времени после CPU profile и boot trace. | Типовой x86-64 путь, но точные MSR/interrupt/timer requirements Windows build ещё не сняты. Скорость не использовалась как аргумент. |
| PCIe | Один root complex, один сегмент и статическая топология; PCIe Base 7.1 программный интерфейс, ACPI MCFG для ECAM. Сначала только необходимые BAR, конфигурация и MSI/MSI-X/INTx варианты, реально нужные выбранным устройствам. | Не реализовывать всю PCIe спецификацию. Набор функций и адресные окна определить после выбора device models. |
| Диагностика | Целевой профиль — один 16550A-compatible UART как COM1 (`I/O 0x3F8`, IRQ4); VMHost должен сохранять байты serial output с временными метками. Отдельный собственный wire format для вывода гостевой ОС не вводить. | Это проектная цель. Bounded polling TX slice описан ниже; полный UART, baud rate, IRQ behavior и доступность для выбранных Windows/Linux образов не подтверждены. |
| Boot storage | RAW disk backend в `od-storage`; для гостя — собственный минимальный NVMe controller, PCIe transport. Планировать только Identify, queue, read/write и flush, если их требует Windows/Linux. | NVMe 2.4 Base, NVM Command Set 1.3, NVMe over PCIe Transport 1.4. В Windows Pro WIM, индекс 6, `stornvme.inf` сопоставляет стандартный PCI class ID `PCI\CC_010802`, а service `stornvme` имеет boot-start. UEFI всё равно нужна собственная реализация NVMe Block I/O для доступа к EFI-разделу; её пока нет. Выбор предварительный, скорость не измерялась. |
| Network | Для первого подключённого NIC — модель, совместимая с Intel 82540EM, PCI ID `8086:100E`; host NAT/backend остаётся в host-компоненте. | Выбор подтверждён владельцем. В Windows Pro WIM, индекс 6, есть `nete1g3e.inf` с этим hardware ID. `netkvm.inf` в образе не найден, поэтому VirtIO потребует собственного Windows-драйвера. Device emulation, Linux guest driver, host backend и сетевой сценарий не реализованы и не проверены; выбор сделан по наличию Windows inbox-драйвера. |
| Экран | UEFI GOP framebuffer как firmware/setup/diagnostic output; собственный WDDM 2.0/GPU stack не входит в v0.1 и не подменяется GOP. | Windows 11 заявляет DirectX 12 + WDDM 2.0 в общих требованиях; GOP не доказывает это требование. Сценарий Blender на этапе 0 — только CPU/headless, если выбранная версия это подтвердит. |
| Ввод | Один стандартный PCI xHCI controller (class `0C0330`, xHCI 1.2) с USB HID keyboard и mouse; дополнительных USB функций в v0.1 нет. Собственная UEFI подключает USB keyboard к `EFI_SIMPLE_TEXT_INPUT_PROTOCOL`; Windows Setup и установленная система используют inbox xHCI/HID stack. | В Windows Pro `install.wim` (index 6) и обоих образах `boot.wim` найдены `usbxhci.inf`, `usbhub3.inf` и `input.inf`. INF связывают xHCI с class ID `PCI\CC_0C0330`, а generic HID с USB class `03`; это подтверждает наличие candidate drivers, но не их запуск или работу ввода. Для UEFI нужен собственный pre-boot USB/HID stack и передача состояния устройства при выходе из Boot Services. |
| TPM / Secure Boot state | Целевой security device — собственная vTPM 2.0; device state и UEFI variables принадлежат persistent state VM и входят в согласованные snapshots. | Требования Windows задают TPM 2.0. Сверяться с TPM Library v185 и PC Client PTP 1.07/Errata 1.1; ни реализация, ни соответствие/сертификация не заявлены. |
| Не включено | Audio, USB passthrough, GPU acceleration, nested virtualization, IOMMU, SMP, hot-plug и миграция. | Добавлять по отдельным требованиям и этапам, а не ради полноты виртуальной платы. |

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
