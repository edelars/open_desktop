# Контракт xHCI-контроллера и USB HID-устройств платы (S8-41)

**Статус S8-41: контракт написан, ожидает ревью Opus (effort high).** Кода модели нет: реализация — S8-42 (контроллер) и S8-43 (HID-устройства), потребитель прошивки — S7-34, потребитель Windows — S8-44. Решение о владельце модели (этап 8) принято по рекомендации Opus high 2026-11-21 и записано в [STAGE_8.md](../plans/STAGE_8.md#s8-01-инвентаризировать-предпосылки-с-владельцами); **подтверждение владельца проекта — ожидается**. Документ описывает версию v1 — минимальный профиль для прошивки и inbox-драйверов Windows, а не полный xHCI.

Слова «реализовано» и «наблюдалось» к этому документу не применимы: всё ниже — требования к будущей реализации. Места, где требование опирается на непроверенное утверждение, перечислены в разделе 14.

## 1. Источники и статус сверки

Редакции по плану ([VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) «Ввод», [STAGE_8.md](../plans/STAGE_8.md) S8-41): Extensible Host Controller Interface for USB (xHCI), редакция 1.2; USB 2.0; USB Device Class Definition for HID, редакция 1.11. **Источник: по плану, редакция не сверена.** Текста этих спецификаций в репозитории нет, к тексту спецификаций доступа при написании документа не было. Ссылки на разделы xHCI/USB/HID ниже не даются: любой номер раздела был бы пересказом по памяти.

Что сверено:

- **Linux `drivers/usb/host/xhci.h`, `xhci-port.h`, `xhci-caps.h`, `xhci-ext-caps.h` (`linux-source-7.0.0`, 2026-11-21)** — не текст xHCI 1.2. Сверены смещения и биты: поля HCSPARAMS1/2, HCCPARAMS1, USBCMD, USBSTS, CRCR, CONFIG, регистры Interrupter (IMAN, IMOD, ERSTSZ, ERSTBA, ERDP), биты PORTSC, типы TRB (1–8, 9–23, 32–38), коды завершения 1–36, поля Slot Context и Endpoint Context (скорость, число контекстов, порт корневого хаба, состояние слота и эндпоинта, тип эндпоинта 4 и 7, Max Packet Size, Interval), положение Slot ID (биты 31:24) и Endpoint ID (биты 20:16) в TRB, формат записи Extended Capability (ID в битах 7:0, Next в битах 15:8). Это сверка совпадения с работающим драйвером; она не заменяет сверку с текстом спецификации.
- **По памяти автора, не сверено ни с чем:** значения reset, доступ к битам, порядок операций контроллера, поведение при ошибках, форматы дескрипторов USB и HID, номера Usage в Keyboard/Keypad. Каждое такое место надо сверить с xHCI 1.2, USB 2.0 и HID 1.11 до приёмки S8-41 (пункт 1 раздела 14).
- **Windows:** требования inbox-стека (`usbxhci`, `usbhub3`, `hidusb`, `kbdhid`, `mouhid`) к MSI/MSI-X, к Extended Capabilities, к дескриптору указателя — по документации Microsoft не проверялись. В [VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) и [WINDOWS_PLATFORM_REQUIREMENTS.md](WINDOWS_PLATFORM_REQUIREMENTS.md) (I-02) подтверждено только наличие INF: `usbxhci.inf`, `usbhub3.inf`, `input.inf`, привязка xHCI к `PCI\CC_0C0330`. Остальное проверяет S8-44.
- **Не копировать** модели QEMU, Bochs, VirtualBox и их дескрипторы отчётов ([AGENTS.md](../../AGENTS.md), S8-41). Формат boot-отчёта клавиатуры задан HID 1.11 (Appendix B) и поэтому единственный; дескрипторы отчётов составлены в разделе 10 из элементов (item) по грамматике HID, а их байты в S8-43 строит кодировщик из таблицы элементов, а не вставленный готовый массив.

## 2. Размещение

| Параметр | Значение |
|---|---|
| PCI-функция | `00:05.0` (шина 0, устройство 5, функция 0); слоты 0:3 (тестовый endpoint) и 0:4 (NVMe) заняты ([NVME_DEVICE.md](NVME_DEVICE.md) §2). Многофункциональность не используется. |
| Идентификаторы | Vendor `0xD1A6` (зарезервированный проектный идентификатор, как у NVMe-endpoint), Device `0x0010`, Revision `0x01`, Subsystem ID `0`. Драйверы привязываются по классу, не по vendor. |
| Класс | `0x0C0330`: base class `0x0C` (serial bus), subclass `0x03` (USB), programming interface `0x30` (xHCI). |
| BAR0 | **64-битный non-prefetchable memory BAR (BAR0/1), размер `0x4000` (16 KiB), запрограммирован при создании платы на `0xFC00_8000`: окно `[0xFC00_8000, 0xFC00_C000)`.** Остальные BAR — RO 0. |
| Почему этот адрес | Карта платы ([FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) 2, [MACHINE_CONTRACT.md](MACHINE_CONTRACT.md) «ECAM и conf1 на плате», [NVME_DEVICE.md](NVME_DEVICE.md) §2) занимает: регистры дисплея `[0xFC00_0000, 0xFC00_1000)`, окно NVMe `[0xFC00_1000, 0xFC00_3000)`, BAR NVMe `[0xFC00_4000, 0xFC00_8000)`, ECAM `[0xFC10_0000, 0xFC20_0000)`. Окно `[0xFC00_8000, 0xFC00_C000)` вплотную за BAR NVMe не пересекает ни одно из них, лежит в том же блоке 2 MiB (число страниц `BOOT_PAGE_TABLES` не меняется, S8-42 это проверяет). Он находится внутри диапазона `[0xFC00_3000, 0xFEC0_0000)`, который [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) не назначает: окна HPET и ACPI fixed hardware выбирает S7-14 вне перечисленных диапазонов, S8-42 сверяет итоговую карту на отсутствие пересечения. Адрес — решение v1, не требование xHCI. |
| Декодирование | Как у NVMe PCI endpoint: Command.MSE (бит 1) включает MMIO, если BAR0 равен запрограммированному адресу; запись другого адреса в BAR0 выключает декодирование (модель не перемещается). Command.BME (бит 2) разрешает DMA; без BME модель не читает и не пишет гостевую RAM, doorbell запоминается и исполняется после включения BME. Command.IO (бит 0) не реализован (RO 0). |
| Прерывание | Interrupt Pin = INTA, level, active-low, фиксированный маршрут **INTA → GSI 17** (у NVMe — GSI 16). Значение вносится в `_PRT` вместе с окном BAR0 в `PCI0._CRS` (ACPI, [ACPI_SMBIOS_TABLES.md](ACPI_SMBIOS_TABLES.md)) в S8-42. Command.INTx Disable (бит 10) снимает линию; Status.Interrupt Status (бит 3) отражает её состояние. |
| Capabilities | **Список capability пуст** (Capabilities Pointer = 0, Status.Capabilities List = 0). MSI и MSI-X в v1 не объявляются (раздел 11). |
| Handoff block | Нового кода устройства в handoff v1 **нет**: прошивка находит xHCI перечислением PCI через ECAM (код 7, [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) 6.3), как и любую PCI-функцию. Требуется `pci_ecam = true`. Нужен ли регион `MMIO` для BAR0 в таблице регионов handoff и в `MachineMemoryMap`, решает S8-42 по образцу PCI-режима NVMe; в любом случае окно обязано входить в единую карту платы и в `_CRS`. |
| Включение | Опционально: `FirmwareBoardConfig::xhci` (имя ориентировочное). По умолчанию функции нет, чтение конфигурации `00:05.0` даёт все единицы, handoff и карта не меняются. |
| Сброс PCI-функции (FLR) | Не реализован (как у NVMe). |

## 3. Модель обслуживания (DMA, время)

Как у NVMe ([NVME_DEVICE.md](NVME_DEVICE.md) §3): `BusDevice::read/write` не имеют доступа к гостевой RAM. Запись в doorbell и в регистры кольца только запоминает состояние и выставляет флаг. Команды, трансферы и события исполняются вызовом `service(&mut MachineBus, committed_write)` **между квантами гостя**; один вызов обрабатывает не более 256 TRB на кольцо и не более 64 событий ввода, остаток — в следующем вызове. Следствия:

- завершение видно гостю не сразу после doorbell, а после ближайшего `service`; драйвер, опрашивающий фазовый бит события (прошивка), это допускает; прерывания INTx приходят после `service`;
- DMA разрешён только в RAM (`MemoryRegionKind::Ram`); любой указатель на MMIO, ROM, framebuffer, вне карты или в окно самого xHCI — ошибка раздела 12, а не доступ и не паника;
- записи в RAM сообщают диапазон в `committed_write`;
- время — virtual clock этапа 4: от него зависят `MFINDEX` и задержка сброса порта (раздел 6). Реального времени и `sleep` в модели нет.

## 4. Capability- и Operational-регистры

Адреса — смещения от BAR0. Capability-регистры читаются с шириной 1, 2, 4 байта (`CAPLENGTH` — байт, `HCIVERSION` — слово), остальные регистры — шириной 4; 64-битные регистры (`CRCR`, `DCBAAP`, `ERSTBA`, `ERDP`) — шириной 4 (две половины, младшая первой) или 8. Запись шириной 1 или 2, запись 8 байт в не-64-битный регистр, невыровненный доступ — ошибка шины (`DeviceError::UnsupportedWidth` или `Misaligned`) до любого изменения состояния. Чтение неопределённого смещения внутри BAR даёт 0, запись игнорируется. Запись в RO-регистр и в зарезервированные биты игнорируется; чтения без побочных эффектов.

### 4.1. Capability

| Смещение | Регистр | Значение v1 |
|---|---|---|
| `0x00` (байт) | CAPLENGTH | `0x20` |
| `0x02` (слово) | HCIVERSION | `0x0120` |
| `0x04` | HCSPARAMS1 | `0x0200_0108`: MaxSlots = 8, MaxIntrs = 1, MaxPorts = 2 |
| `0x08` | HCSPARAMS2 | `0`: IST = 0, ERST Max = 0 (один сегмент таблицы событий), scratchpad-буферов нет |
| `0x0C` | HCSPARAMS3 | `0` (U1/U2 latency, USB 3 не поддерживается) |
| `0x10` | HCCPARAMS1 | `0x0800_0001`: AC64 = 1 (64-битная адресация), CSZ = 0 (контексты по 32 байта), PPC = 0, остальное 0; xECP = `0x800` (в dword, т. е. смещение `0x2000`) |
| `0x14` | DBOFF | `0x0000_0800` |
| `0x18` | RTSOFF | `0x0000_1000` |
| `0x1C` | HCCPARAMS2 | `0` |

### 4.2. Operational (база `0x20`)

| Смещение | Регистр | Доступ, reset, поведение |
|---|---|---|
| `0x20` | USBCMD | Реализованы биты: R/S (0), HCRST (1, самоочищающийся, читается 0), INTE (2), HSEE (3). Остальные (LHCRST, CSS, CRS, EWE, EU3S, CME, ETE и пр.) — RO 0. Reset `0`. R/S: 0→1 включает обработку колец и сбрасывает USBSTS.HCH; 1→0 останавливает обработку (команды и трансферы не исполняются), сбрасывает CRCR.CRR и выставляет HCH сразу (в том же доступе). Запись R/S = 1 при USBSTS.HSE = 1 игнорируется до HCRST. |
| `0x24` | USBSTS | HCH (0) RO, reset 1; HSE (2) RW1C; EINT (3) RW1C; PCD (4) RW1C; SSS, RSS (8, 9) RO 0; SRE (10) RW1C, никогда не выставляется; CNR (11) RO, всегда 0 (сброс мгновенный); HCE (12) RO 0. Reset `0x0000_0001`. |
| `0x28` | PAGESIZE | RO `0x0000_0001` (4 KiB). |
| `0x34` | DNCTRL | RW биты 15:0, хранятся, на поведение не влияют (Device Notification не доставляются). Reset 0. |
| `0x38` (64) | CRCR | Запись: RCS (0) и Command Ring Pointer (63:6) принимаются **только при CRR = 0**; CS (1) и CA (2) в v1 не реализованы (читаются 0, запись игнорируется, раздел 13). Чтение: бит 3 CRR, остальные 0 (указатель при чтении 0). CRR выставляется первым doorbell 0 при R/S = 1 и сбрасывается R/S = 0 и HCRST. Reset 0. |
| `0x50` (64) | DCBAAP | Биты 63:6, нижние 6 бит 0. Запись принимается только при HCH = 1; иначе игнорируется. Reset 0. |
| `0x58` | CONFIG | MaxSlotsEn (7:0), U3E и CIE (8, 9) RO 0. Запись только при HCH = 1. Эффективное число слотов — `min(MaxSlotsEn, 8)`; Enable Slot при `MaxSlotsEn = 0` даёт No Slots Available. Reset 0. |

## 5. Runtime, Interrupter 0 и Doorbell

| Смещение | Регистр | Доступ, reset, поведение |
|---|---|---|
| `0x1000` | MFINDEX | RO, 14 бит: `floor(virtual_ns / 125_000) & 0x3FFF`; MFINDEX Wrap Event не формируется. |
| `0x1020` | IMAN | IP (0) RW1C, IE (1) RW; reset 0. IP выставляется при записи в кольцо событий (кроме событий от TRB с BEI = 1), вместе с ней USBSTS.EINT. |
| `0x1024` | IMOD | IMODI (15:0) reset `0x0FA0`, IMODC (31:16) RW; значения хранятся, замедления доставки в v1 нет. |
| `0x1028` | ERSTSZ | Биты 15:0. Запись значения > 1 игнорируется (ERST Max = 0). Принимается только при HCH = 1. |
| `0x1030` (64) | ERSTBA | Биты 63:6. Запись только при HCH = 1; запись загружает единственную запись ERST (16 байт: база сегмента 64 бита, размер в TRB 32 бита) и инициализирует кольцо: enqueue = начало сегмента, cycle = 1. Размер сегмента вне `[16, 4096]`, база не кратна 64 или не в RAM — **HSE** (раздел 12). |
| `0x1038` (64) | ERDP | DESI (2:0) RW (0 — единственный допустимый сегмент), EHB (3) RW1C, указатель (63:4). Запись нового dequeue освобождает место в кольце. EHB = 1 при первом событии, пока драйвер не очистит. |
| `0x800 + 4*n` | Doorbell n, `n = 0..8` | Только запись, чтение 0. n = 0 — командное кольцо (DB Target = 0, иначе игнорируется). n = 1..8 — слот n: DB Target (7:0) = Endpoint ID (DCI) 1..31, DB Stream ID (31:16) обязан быть 0. Doorbell при HCH = 1, в выключенный слот или на DCI не из `{1, 3}` игнорируется со счётчиком диагностики. Без Command.BME doorbell запоминается и исполняется после включения BME. |

**Кольцо событий.** TRB пишутся в сегмент с текущим cycle bit; при достижении конца сегмента cycle инвертируется и enqueue возвращается в начало. Для событий резервируется последний свободный TRB: когда остаётся один слот, модель пишет в него **Event Ring Full Error** (Host Controller Event, тип 37, код завершения 21; тип события — по памяти, раздел 14) один раз и перестаёт обрабатывать команды, трансферы и сообщать о портах, пока драйвер не продвинет ERDP. Внутренней очереди событий нет: события формируются на месте из состояния (команда в кольце, TRB в кольце трансфера, бит изменения PORTSC), поэтому ничего не теряется, кроме отложенного исполнения. Прерывание INTx: пока выставлены IMAN.IP, IMAN.IE и USBCMD.INTE и не выставлен Command.INTx Disable, линия удерживается; запись 1 в IMAN.IP снимает её (EINT очищается записью 1 в USBSTS.EINT независимо).

## 6. Порты и корневой хаб

Два порта USB 2.0 (MaxPorts = 2, оба в одной Supported Protocol, раздел 7), без внешних хабов: **порт 1 — клавиатура, порт 2 — абсолютный указатель**. Устройства подключены с момента сброса платы; горячее подключение и отключение в v1 нет. Порты `PORTSCn` — `0x420 + 0x10*(n-1)`: PORTSC, PORTPMSC (+4), PORTLI (+8), PORTHLPMC (+0xC). PORTPMSC, PORTLI, PORTHLPMC — RO 0.

PORTSC (по Linux `xhci-port.h`):

| Бит | Поле | Поведение в v1 |
|---|---|---|
| 0 | CCS | RO; 1 всегда (устройство подключено). |
| 1 | PED | Читается 0 до сброса порта и 1 после; запись 1 отключает порт (PED = 0, PLS = 7 Polling, PEC = 1), порт требует нового сброса. |
| 3 | OCA | RO 0. |
| 4 | PR | Запись 1 при CCS = 1 и PR = 0 запускает сброс: PR = 1, PED = 0, PLS = 7. Через **10 мс virtual clock** (константа `USB_PORT_RESET_NS = 10_000_000`; в тестах допускается 0 по явной настройке) модель завершает сброс: PR = 0, PED = 1, PLS = 0 (U0), PRC = 1, устройство возвращается в состояние Default (адрес 0, не сконфигурировано, протокол по умолчанию, idle 0, очередь отчётов очищена; слоты, привязанные к порту, не отключаются, их трансферы завершаются **USB Transaction Error (4)**, пока Address Device не повторён). |
| 8:5 | PLS | Чтение: 0 (U0) при PED = 1, 7 (Polling) до сброса, 4 (Disabled) не используется. Запись с LWS (бит 16) = 1: значение 0 принимается как no-op; **U2, U3 и Resume игнорируются** (порт остаётся U0; приостановка устройств вне v1). LWS читается 0. |
| 9 | PP | RO 1 (PPC = 0). |
| 13:10 | Port Speed | RO; **1 (Full Speed)** при CCS = 1. Оба устройства — Full-speed, 12 Мбит/с. |
| 15:14 | PIC | RO 0 (индикаторов нет). |
| 17 | CSC | RW1C; reset после HCRST — 1 (подключение, ещё не отчитанное). |
| 18 | PEC | RW1C; 1 при отключении порта записью PED. |
| 21 | PRC | RW1C; 1 по завершению сброса. |
| 19, 20, 22, 23 | WRC, OCC, PLC, CEC | RW1C, никогда не выставляются. |
| 24 | CAS | RO 0. |
| 25–27, 30, 31 | WCE, WDE, WOE, DR, WPR | RO 0 (пробуждение и warm reset не поддерживаются). |

**Port Status Change Event.** Для каждого порта модель хранит «изменение отчитано»: при появлении любого из CSC, PEC, PRC (переход 0→1), когда R/S = 1 и кольцо событий настроено, один раз публикуется Port Status Change Event (тип 34, Port ID в DW0 31:24, код Success в DW2 31:24) и выставляется USBSTS.PCD. Если событие нельзя записать (R/S = 0, кольцо не настроено, кольцо заполнено), флаг остаётся и событие публикуется при первой возможности. Запись 1 в бит изменения очищает его (и снимает «отчитано»). Драйвер, опрашивающий PORTSC, события не обязан читать (прошивка).

## 7. Extended Capabilities

С смещения `0x2000` (xECP = `0x800`) единственная запись — **Supported Protocol Capability** (ID 2, 16 байт, Next = 0):

| Смещение | Значение |
|---|---|
| `0x2000` | `0x0200_0002`: ID = 2, Next = 0, Revision Minor = `0x00`, Revision Major = `0x02` (USB 2.0) |
| `0x2004` | `0x2042_5355` — строка «USB » |
| `0x2008` | `0x0000_0201`: Compatible Port Offset = 1, Compatible Port Count = 2, PSIC = 0 (скорости по умолчанию), Protocol Defined = 0 |
| `0x200C` | `0x0000_0000`: Protocol Slot Type = 0 |

USB Legacy Support (ID 1) и BIOS-владение **не объявляются**: передавать управление прошивки ОС через USBLEGSUP не нужно, остановку контроллера прошивкой описывает раздел 12, потребителя — раздел 15. Диапазон `0x2010–0x3FFF` читается 0; `0x3000–0x3FFF` зарезервирован под таблицу MSI-X и PBA на случай расширения (раздел 11).

## 8. Структуры данных в гостевой RAM

Все структуры — в гостевой RAM по физическим адресам, которые пишет драйвер. Адрес должен быть в RAM, кольца и контексты — выровнены (ниже), иначе раздел 12.

- **DCBAA** (Device Context Base Address Array): 64-битные указатели, элемент 0 не используется (scratchpad нет), элемент n — Output Device Context слота n. Выравнивание 64, размер `9 * 8` байт.
- **Контексты по 32 байта** (CSZ = 0): Device Context = Slot Context + 31 Endpoint Context (1024 байта, выравнивание 64); Input Context = Input Control Context (32 байта) + Device Context (1056 байт, выравнивание 64). Endpoint Context для DCI n — индекс n в Device Context (DCI 1 — EP0, DCI 3 — EP1 IN).
- **Поля, которые использует модель.** Slot Context: DW0 — Route String (19:0, должна быть 0), Speed (23:20), Context Entries (31:27); DW1 — Root Hub Port Number (23:16), Max Exit Latency (15:0); DW2 — Interrupter Target (31:22, должен быть 0); DW3 — USB Device Address (7:0), Slot State (31:27: 0 Disabled/Enabled, 1 Default, 2 Addressed, 3 Configured). Endpoint Context: DW0 — EP State (2:0: 0 Disabled, 1 Running, 2 Halted, 3 Stopped, 4 Error), Mult и MaxPStreams (должны быть 0), Interval (23:16); DW1 — Error Count (2:1), EP Type (5:3: 4 Control, 7 Interrupt IN), Max Burst (15:8, 0), Max Packet Size (31:16); DW2:DW3 — TR Dequeue Pointer (бит 0 — DCS); DW4 — Average TRB Length. Остальные поля хранятся как записаны и не интерпретируются.
- **TRB** — 16 байт, выравнивание 16. Общая раскладка: DW0:DW1 — параметр; DW2 — статус; DW3 — управление: Cycle (0), ENT/TC (1), ISP (2), CH (4), IOC (5), IDT (6), BSR/DC (9), TRB Type (15:10), DIR (16), Slot ID (31:24), Endpoint ID (20:16). Командное кольцо и кольца трансферов — сегменты по 256 TRB рекомендуемо, ≥ 2 TRB, один сегмент не пересекает границу 64 KiB; кольцо замыкает **Link TRB** (тип 6) с Toggle Cycle. Цепочки из нескольких сегментов через Link допустимы; Event Data TRB (7) и Transfer No-Op (8) не поддерживаются (**TRB Error**).
- **Кольцо событий** — единственный сегмент через ERST (раздел 5), 16–4096 TRB.

## 9. Команды, трансферы и события

### 9.1. Команды v1

Командное кольцо исполняется при doorbell 0 в порядке кольца до первого TRB, чей Cycle ≠ RCS (кольцо пусто). Каждая команда порождает один **Command Completion Event** (тип 33): DW0:DW1 — физический адрес TRB команды, DW2 — код завершения в 31:24 и параметр 0, DW3 — Slot ID (31:24), VF ID 0, тип 33, Cycle. Link TRB в командном кольце обрабатывается без события. Типичный порядок драйвера: Enable Slot → Address Device → (Configure Endpoint) → Evaluate Context при необходимости.

| Команда (TRB Type) | Поведение и коды завершения |
|---|---|
| **Enable Slot (9)** | Выбирает наименьший свободный Slot ID в `1..min(MaxSlotsEn, 8)`, Slot Type (20:16) должен быть 0. Нет свободного — **No Slots Available (9)**. Иначе **Success (1)** с Slot ID, слот в состоянии Enabled; Output Context не пишется до Address Device. |
| **Disable Slot (10)** | *Добавлена к списку из постановки S8-41:* без неё слот не освободить (восьми слотов хватает на два устройства, но порт-сброс и повторное перечисление требуют освобождения). Слот не включён — **Slot Not Enabled (11)**; иначе слот Disabled, DCBAA[n] не трогается (RAM драйвера), Success. |
| **Address Device (11)** | Параметр — указатель на Input Context (выравнивание 64, иначе **TRB Error (5)**), BSR (9), Slot ID (31:24). Слот не включён — **Slot Not Enabled (11)**. Состояние слота не Enabled/Default (BSR = 0) или не Enabled (BSR = 1) — **Context State Error (19)**. Input Control Context: A0 = A1 = 1, остальные A и все D равны 0, иначе **Parameter Error (17)**. Slot Context: Root Hub Port Number ∈ {1, 2}, Route String = 0, Interrupter Target = 0, Context Entries ≥ 1; EP0 Context: тип 4, Max Packet Size ≥ 8, TR Dequeue Pointer ≠ 0 и кратен 16, иначе **Parameter Error (17)**. Порт без подключённого устройства, не включён (PED = 0) — **USB Transaction Error (4)**. DCBAA[slot] = 0 или не в RAM — HSE (раздел 12). BSR = 0: устройству посылается SET_ADDRESS (адрес = Slot ID), Slot State = Addressed, Address в Slot Context = Slot ID; BSR = 1: адрес 0, Slot State = Default, SET_ADDRESS не посылается. Модель копирует Slot и EP0 Context в Output Device Context (EP State = Running, Speed = 1, порт и Context Entries — из входа). Success. |
| **Configure Endpoint (12)** | Слот не Addressed/Configured — **Context State Error (19)**; Slot Not Enabled — (11). D0, D1 и A1 обязаны быть 0; `A0` допускается (Context Entries). Поддерживается добавление только DCI 3 (Interrupt IN) с теми же проверками, что у EP0 (тип 7, Max Packet Size ≥ длины отчёта устройства — 8 для клавиатуры, 6 для указателя; Interval 1..16; TR Dequeue Pointer ≠ 0, кратен 16); добавление другого DCI, второе добавление уже запущенного DCI (без Drop) и Max Burst/Mult/MaxPStreams ≠ 0 — **Parameter Error (17)**. Drop DCI 3 допустим. Бит **DC** (Deconfigure, 9) возвращает слот в Addressed и отключает DCI 3. Успех: слот Configured (или Addressed при пустой конфигурации), DCI 3 Running, Success. Учёт пропускной способности отсутствует: **Bandwidth Error (8)** и **Resource Error (7)** не формируются. |
| **Evaluate Context (13)** | Слот Default, Addressed или Configured, иначе **Context State Error (19)**. Применяются A0 (Max Exit Latency, Interrupter Target = 0) и A1 (Max Packet Size EP0); D ≠ 0 или A для DCI ≥ 2 — **Parameter Error (17)**. Success. |
| **No Op (23)** | Success без других действий. |
| Любой другой тип | **TRB Error (5)** с Slot ID из TRB, состояние не меняется. К ним относятся *Stop Endpoint (15), Reset Endpoint (14), Set TR Dequeue Pointer (16), Reset Device (17)*, Force Event, Negotiate Bandwidth, Set Latency Tolerance Value, Get Port Bandwidth, Force Header, Get/Set Extended Property (раздел 13). |

DMA-ошибка при чтении Input Context или записи Output Context — HSE (раздел 12), а не код завершения. Каждая строка таблицы — отдельный обязательный тест S8-42, как и каждый отказ.

### 9.2. События

| Событие | Тип | Содержимое |
|---|---|---|
| Command Completion | 33 | раздел 9.1 |
| Port Status Change | 34 | DW0 31:24 — Port ID (1 или 2); DW2 31:24 — Success (1); DW3: тип 34, Cycle |
| Transfer | 32 | DW0:DW1 — указатель на TRB, породивший событие; DW2: код (31:24), остаток в байтах (23:0) — разность запрошенной и фактической длины; DW3: Slot ID (31:24), Endpoint ID (20:16), тип 32, Cycle |
| Host Controller (Event Ring Full) | 37 | код 21, раздел 5 |

Каждое событие пишется как 4 dword, **последним — DW3 с Cycle** (запись разрешённой записью по адресу; частично записанный TRB гость не видит). После записи — IMAN.IP, USBSTS.EINT (раздел 5), INTx по условиям раздела 5. Событие от TRB с BEI = 1 прерывание не выставляет.

### 9.3. Трансферы

Doorbell слота n с DCI 1 (EP0) или 3 (EP1 IN) запускает исполнение кольца этого эндпоинта, если слот Addressed/Configured и эндпоинт Running. TR Dequeue Pointer и DCS берутся из контекста эндпоинта. TRB Cycle ≠ DCS — кольцо пусто. Stream ID, Interrupter Target и Block Event Interrupt обрабатываются: Stream ID ≠ 0 и Interrupter Target ≠ 0 — **TRB Error (5)**.

**Control (EP0, DCI 1).** Транзакция состоит из TD: **Setup Stage (тип 2)**, необязательный **Data Stage (3)**, **Status Stage (4)**. Setup: IDT = 1, длина 8, байты запроса в DW0:DW1 (bmRequestType, bRequest, wValue, wIndex, wLength), TRT (17:16): 0 — без данных, 2 — OUT, 3 — IN; несоответствие TRT и wLength или IDT = 0 — **TRB Error (5)**. Data Stage: один TRB (цепочка CH = 1 не поддерживается — **TRB Error**), направление DIR должно совпадать с TRT и битом 7 bmRequestType, длина ≤ 65535; Status Stage: DIR противоположен данным (IN, если данных нет). Запрос исполняет устройство (раздел 10): IN-данные записываются в буфер Data Stage ровно `min(wLength, длина ответа, длина TRB)` байт. Короткий ответ: при ISP = 1 событие Data Stage с кодом **Short Packet (13)** и остатком, при ISP = 0 события нет; в обоих случаях исполнение идёт к Status Stage. События — для каждого TRB с IOC = 1: Success (1) (Setup Stage по IOC; Data — с остатком; Status). Ответ устройства STALL — **Stall Error (6)** для TD, EP0 переходит в **Halted (2)**, и в v1 из Halted нет выхода, кроме Disable Slot / сброса порта и повторного перечисления (раздел 14, пункт 3). Буфер вне RAM — **Data Buffer Error (2)**, эндпоинт Halted. Слот, чьё устройство не находится в состоянии, соответствующем контексту (после сброса порта) — **USB Transaction Error (4)**.

**Interrupt IN (DCI 3).** Кольцо из **Normal TRB (тип 1)**: адрес буфера, длина (≥ длины отчёта), IOC, ISP, BEI. Пока у устройства нет отчёта, TRB не потребляется (аналог NAK, события нет). Когда отчёт готов, модель записывает его в буфер, потребляет TRB и формирует Transfer Event: Success (1), остаток = длина TRB − длина отчёта (при ISP = 1 и остатке > 0 — **Short Packet (13)**). Длина TRB меньше длины отчёта — **Babble Detected Error (3)**, эндпоинт Halted (выхода нет, раздел 14). Интервал опроса (Interval) и `SET_IDLE` не используются для задержек: отчёт отдаётся при ближайшем `service` после его появления, не чаще одного отчёта на TRB.

## 10. USB-устройства и HID

Два устройства, оба Full-speed, подключены к портам 1 и 2 (раздел 6). Общие свойства: bcdUSB `0x0200`, bDeviceClass/SubClass/Protocol 0 (класс — в интерфейсе), bMaxPacketSize0 64, idVendor `0xD1A6`, bcdDevice `0x0100`, iManufacturer 1, iProduct 2, iSerialNumber 0, bNumConfigurations 1. Строки: индекс 0 — LANGID `0x0409`; 1 — «Open Desktop»; 2 — «Virtual HID Keyboard» / «Virtual HID Absolute Pointer» (UTF-16LE). Конфигурация: bConfigurationValue 1, bmAttributes `0x80` (питание от шины, без remote wakeup), bMaxPower 50 (100 мА), один интерфейс класса HID (3), одна конечная точка Interrupt IN (адрес `0x81`, bmAttributes `0x03`, bInterval 10), wTotalLength 34 (9 + 9 + 9 + 7).

| | Клавиатура (порт 1) | Абсолютный указатель (порт 2) |
|---|---|---|
| idProduct | `0x0101` | `0x0102` |
| Интерфейс | subclass 1 (boot), protocol 1 (keyboard) | subclass 0, protocol 0 |
| HID descriptor | bcdHID `0x0111`, country 0, один descriptor типа Report (0x22), длина **63** | то же, длина **63** |
| wMaxPacketSize | 8 | 6 |
| Отчёт IN | 8 байт | 6 байт |
| Отчёт OUT | 1 байт (LED) | нет |

### 10.1. Стандартные запросы (оба устройства)

Через control EP0: **GET_STATUS** (устройство, интерфейс, эндпоинт — 0x0000); **GET_DESCRIPTOR** — устройство (1), конфигурация индекса 0 (2, весь комплект 34 байта), строка (3), HID descriptor интерфейса (0x21), Report descriptor интерфейса (0x22); **GET_CONFIGURATION**, **SET_CONFIGURATION** (0 — нет конфигурации, 1 — настроено, иначе STALL), **GET_INTERFACE** (0), **SET_INTERFACE** (только alt 0), **SET_ADDRESS** (выполняет контроллер по Address Device; control-TRB с этим запросом — **TRB Error**). **Строка с неизвестным индексом (включая 0xEE, запрос Microsoft OS String)** получает дескриптор строки длиной 2 (`bLength = 2, bDescriptorType = 3`) вместо STALL: STALL приводит к Halted EP0 без Reset Endpoint (раздел 14). Поведение Windows на такой ответ не сверено. Всё остальное — **STALL**: Device_Qualifier (6), Other_Speed_Configuration (7), BOS (15), Physical (0x23), SET_DESCRIPTOR, SYNCH_FRAME, CLEAR_FEATURE/SET_FEATURE (ENDPOINT_HALT, DEVICE_REMOTE_WAKEUP) и неизвестные запросы. Запросы в состоянии Default (адрес 0) допустимы после Address Device с BSR = 1.

Отчёты формируются только пока выполнены оба условия: SET_CONFIGURATION(1) и DCI 3 Running (порядок Configure Endpoint и SET_CONFIGURATION произвольный). Внутреннее состояние (нажатые клавиши, координаты) обновляется всегда; очередь отчётов существует только пока эндпоинт запущен. После SET_CONFIGURATION/Configure Endpoint отчёт автоматически не отправляется: первый отчёт — по следующему изменению состояния.

### 10.2. HID-запросы класса (интерфейс 0)

| bRequest | Клавиатура | Указатель |
|---|---|---|
| GET_REPORT (0x01) | Input (тип 1, ID 0): текущее 8-байтное состояние; Output (тип 2): байт LED; Feature — STALL | Input: текущее 6-байтное состояние; прочие — STALL |
| SET_REPORT (0x09) | Output (тип 2): 1 байт LED, хранится и доступен тестовому API (раздел 16); Input и Feature — STALL | STALL |
| GET_IDLE (0x02) / SET_IDLE (0x0A) | значение (в единицах 4 мс) хранится и возвращается; периодической повторной отправки в v1 нет | то же |
| GET_PROTOCOL (0x03) / SET_PROTOCOL (0x0B) | 0 — boot, 1 — report; reset-значение **1**; байты отчёта в обоих протоколах одинаковы (дескриптор boot-совместим, Report ID нет) | STALL (интерфейс не boot) |

Reset-значение idle — 0. Всё прочее — STALL (раздел 14, пункт 3).

### 10.3. Клавиатура: дескриптор отчёта и отчёт

Report descriptor (Usage Page Generic Desktop, Usage Keyboard, Application Collection) состоит из элементов, в порядке: Usage Page(0x07) → Usage Min 0xE0 / Max 0xE7 → Logical Min 0 / Max 1 → Report Size 1, Count 8 → **Input** (Data, Var, Abs) — модификаторы; Report Count 1, Size 8 → **Input** (Const) — зарезервированный байт; Report Count 5, Size 1, Usage Page LEDs(0x08), Usage Min 1 / Max 5 → **Output** (Data, Var, Abs); Report Count 1, Size 3 → **Output** (Const) — добивка; Report Count 6, Size 8, Logical Min 0 / Max 0xE7, Usage Page(0x07), Usage Min 0 / Max 0xE7 → **Input** (Data, Array); End Collection. Это 63 байта; значение wDescriptorLength = 63 (`0x003F`). Тест S8-43 сверяет кодированные байты и структуру с HID 1.11 Appendix B и фиксирует результат.

**Отчёт IN (8 байт):** байт 0 — модификаторы (бит 0 LeftCtrl 0xE0 … бит 7 RightGUI 0xE7), байт 1 — 0, байты 2–7 — Usage ID нажатых немодификаторных клавиш в порядке нажатия, свободные — 0. **Отчёт OUT (1 байт):** бит 0 NumLock, 1 CapsLock, 2 ScrollLock, 3 Compose, 4 Kana, биты 5–7 — 0.

**События ввода → отчёт.** Источник — очередь событий ввода машины (раздел 16); событие `Key { usage: u8, pressed: bool }`, где `usage` — **USB HID Usage ID страницы Keyboard/Keypad (0x07)**, не PS/2 скан-код: допустимы `0x04..=0xDD` (клавиши) и `0xE0..=0xE7` (модификаторы); `0x00..=0x03`, `0xDE..=0xDF`, `> 0xE7` — отбрасываются со счётчиком. Повторное нажатие уже нажатой и отпускание не нажатой клавиши — без отчёта. Каждое изменение множества нажатых клавиш формирует один отчёт (полный снимок состояния). Если одновременно нажато более 6 немодификаторных клавиш, байты 2–7 содержат `0x01` (ErrorRollOver) до тех пор, пока их не станет ≤ 6, байт 0 при этом сохраняется. Таблица соответствия клавиш окна Usage ID принадлежит S10-16; редакция HID Usage Tables здесь не фиксируется (раздел 14).

### 10.4. Абсолютный указатель: дескриптор отчёта и отчёт

**Выбор:** Generic Desktop → **Mouse** (application collection) → Physical collection **Pointer**, три кнопки, оси X и Y **абсолютные 0..32767** по 16 бит, колесо относительное. Альтернатива — Digitizer (usage page 0x0D). Выбор сделан без документации Microsoft «HID over USB»; пригодность для inbox `mouhid` и точность координат проверяет S8-44 (раздел 14, пункт 4); при отказе документ переиздаётся с Digitizer. Элементы в порядке: Usage Page(Generic Desktop), Usage(Mouse), Collection(Application); Usage(Pointer), Collection(Physical); Usage Page(Button), Usage Min 1 / Max 3, Logical 0..1, Size 1, Count 3, **Input** (Data, Var, Abs); Count 1, Size 5, **Input** (Const); Usage Page(Generic Desktop), Usage(X), Usage(Y), Logical 0..32767, Size 16, Count 2, **Input** (Data, Var, Abs); Usage(Wheel), Logical −127..127, Size 8, Count 1, **Input** (Data, Var, **Rel**); End Collection ×2. Всего 63 байта.

**Отчёт IN (6 байт):** байт 0 — кнопки (бит 0 левая, 1 правая, 2 средняя; биты 3–7 — 0), байты 1–2 — X (little-endian), 3–4 — Y, байт 5 — колесо (знаковый, приращение с момента прошлого отчёта). События: `PointerAbs { x: u16, y: u16 }` (значения > 32767 — отбрасываются со счётчиком), `Buttons { mask: u8 }` (биты 3–7 игнорируются), `Wheel { delta: i8 }`. Колесо накапливается и выдаётся порциями −127..127, остаток переносится; после отчёта накопление 0. Каждое изменение формирует отчёт. **Относительная мышь в v1 не реализуется**; захват курсора и relative mode — вне v1 (S10-16 использует абсолютный указатель). Производитель событий обязан сам перевести пиксели гостя в 0..32767 по формуле `x_logical = round(x_px * 32767 / (width − 1))` (и так же y): соответствие этого диапазона отображению Windows на пиксели экрана не сверено (S8-44, допуск ≤ 1 гостевого пикселя).

### 10.5. Переполнение очередей отчётов

Очередь отчётов устройства — не более 16. Отчёты — снимки состояния, поэтому при переполнении новый снимок **заменяет последний** в очереди: промежуточные нажатия теряются (счётчик `reports_coalesced`), итоговое состояние сохраняется. Очередь событий ввода машины — раздел 16.

## 11. Прерывания и опрос; MSI и MSI-X не в v1

В v1 есть два способа получения событий: **опрос** (фазовый бит событий и `ERDP`; прошивка S7-34 работает только так, ставит Command.INTx Disable и USBCMD.INTE = 0) и **INTx** level по GSI 17 (раздел 5) для драйверов, которые используют линию. **MSI и MSI-X не реализуются и не объявляются в списке capability** — решение v1 (рекомендация Opus high 2026-11-21): не усложнять первый срез, прошивке они не нужны. Для Windows это **не сверено**: если `usbxhci` требует MSI или MSI-X (или не работает на линии INTx в этой конфигурации), S8-44 фиксирует отказ, и контракт получает версию v1.1 с MSI-X: capability `0x50`-стиля как у NVMe ([NVME_DEVICE.md](NVME_DEVICE.md) §3.1), таблица и PBA в зарезервированных страницах BAR0 (`0x3000`, раздел 7), вектор Interrupter 0 = вектор 0. Дозированное прерывание (IMOD) в v1 не моделируется.

## 12. Ошибки, HCRST, остановка

### 12.1. Классы ошибок

| Класс | Примеры | Реакция |
|---|---|---|
| **Гостевая ошибка протокола** | неверный TRB, Parameter/Context State/Slot Not Enabled, STALL устройства, буфер данных вне RAM | **Event TRB с кодом завершения** (разделы 9.1–9.3); контроллер продолжает работу; эндпоинт Halted — только для Stall, Babble и Data Buffer Error. |
| **Системная ошибка DMA** | указатель DCBAA, Input/Output Context, ERST, командного кольца или кольца событий не в RAM или не выровнен | **USBSTS.HSE = 1**, R/S принудительно 0, HCH = 1, обработка колец прекращается; при USBCMD.HSEE = 1 — прерывание по линии INTx (если разрешено IMAN/Command), в журнал диагностики модели — причина. Восстановление — HCRST. |
| **Ошибка входа (backend)** | переполнение очереди событий ввода, недопустимый usage или координата, колесо вне диапазона | В гостя не передаётся; событие отбрасывается, производитель получает ошибку (`InputQueueFull` или счётчик отбрасывания). Устройство и контроллер не меняют состояние. |
| **Ошибка хоста/модели** | внутренняя паника, нарушение инварианта | Не гостевое исключение и не HSE: не скрывается перехватом (AGENTS.md). |

### 12.2. HCRST

Запись USBCMD.HCRST = 1 (при любом R/S; HCH к моменту ответа = 1): **всё состояние Operational, Runtime и контекстов сбрасывается** (разделы 4–5, все слоты Disabled, кольца и указатели забываются; DCBAA, ERST и RAM гостя не изменяются), **все порты возвращаются в состояние «подключено, не сброшено»** (CCS = 1, PED = 0, PLS = Polling, CSC = 1), устройства — в Default (адрес 0, не сконфигурированы, idle 0, protocol report, очереди отчётов очищены; состояние нажатых клавиш и координат сбрасывается: все клавиши отпущены, кнопки отжаты, координаты не меняются). HCRST и CNR — мгновенно: HCRST читается 0, CNR = 0 после той же записи. PCI-конфигурация (BAR, Command) не меняется. Сброс платы выполняет то же и возвращает PCI-конфигурацию к значениям при создании.

### 12.3. Остановка при выходе из Boot Services (S7-34)

Прошивка перед передачей управления ОС: R/S = 0, ожидание HCH = 1, HCRST, ожидание HCRST = 0 и CNR = 0, USBCMD.INTE = 0, восстановление Command, прочитанного до начала работы (раздел 15). После этого модель **не читает и не пишет гостевую RAM** (нет кольцевой обработки, нет DMA), регистры в состоянии reset (HCH = 1, CRR = 0, IMAN.IE = 0, DCBAAP = 0); BAR0 неизменен. Тест S7-34/S8-42 проверяет эти значения через регистры модели и счётчик обращений к RAM. ОС (Windows) инициализирует контроллер с reset-состояния; USB Legacy Support не используется.

## 13. Не входит в v1

Явные границы; расширение — отдельные задачи или версия контракта:

- **MSI и MSI-X** (раздел 11); **Stop Endpoint, Reset Endpoint, Set TR Dequeue Pointer, Reset Device** и прочие команды, кроме перечисленных в 9.1 (отказ — TRB Error): последствие — эндпоинт в Halted после Stall/Babble/Data Buffer Error не восстанавливается без Disable Slot или сброса порта. Это отступление от текста плана S8-41 (там перечислены Reset/Stop Endpoint и Set TR Dequeue Pointer), принятое по рекомендации Opus high 2026-11-21; кандидат на v1.1 по итогам S8-44.
- **Command Abort и Command Ring Stop** (CRCR.CA, CRCR.CS): команды исполняются целиком внутри `service`, прерывать нечего; Command Ring Stopped/Aborted не формируются.
- USB 3 / SuperSpeed (в том числе Slot Context speed ≥ 4), SuperSpeed-порты и Supported Protocol USB 3; High-speed и Low-speed устройства; **streams**; **isochronous**; bulk; **mass storage** и прочие классы кроме HID; **внешние хабы** (Route String ≠ 0), TT; **passthrough** USB с host.
- Горячее подключение и отключение, suspend/resume, remote wakeup, Link Power Management, USB 3 Link, управление питанием портов.
- Save/Restore State (USBCMD.CSS/CRS), Light Host Controller Reset, Virtualization/Debug/Legacy Support capabilities, scratchpad-буферы, Device Notification, MFINDEX Wrap Event, Bandwidth Request, Doorbell Event, Force Event, Get Port Bandwidth, Extended Property, Latency Tolerance.
- Event Data TRB, цепочки Data Stage, 64-байтовые контексты (CSZ = 1), несколько Interrupters, несколько сегментов ERST.
- Относительная мышь, Digitizer, multi-touch, Feature reports, периодическая повторная отправка по idle, повтор клавиш (typematic) — повторы делает ОС или прошивка.
- Снимки состояния: состояние xHCI не входит в snapshot ([VM_STATE.md](VM_STATE.md)); задача этапа 9.
- Копирование моделей и дескрипторов QEMU/Bochs/VirtualBox и других сторонних реализаций.

## 14. Не сверено (NOT VERIFIED)

Места, где реализация и тесты могут разойтись с реальными драйверами:

1. **Весь документ** сверен только с заголовками Linux xHCI (раздел 1); смещения, биты, reset-значения, порядок операций, значения IMOD, ERST Max = 0, нулевое число scratchpad, CRR-семантика (CRR остаётся 1 до остановки), тип события Event Ring Full (37), поведение записей в `CONFIG`/`ERSTBA` при R/S = 1 — **по памяти**, до приёмки сверить с xHCI 1.2.
2. **MSI/MSI-X и INTx в Windows** (раздел 11).
3. **STALL и Halted без Reset Endpoint.** В xHCI STALL эндпоинта требует Reset Endpoint; в v1 команды нет. Смягчение: неизвестные строки (0xEE) получают дескриптор длиной 2. Windows может опрашивать и другие дескрипторы (Device_Qualifier, BOS, Other_Speed) — тогда EP0 окажется в Halted, и S8-44 добавит команды в v1.1. Не проверено.
4. **Указатель:** Mouse + абсолютные X/Y 0..32767 вместо Digitizer; соответствие `mouhid`, масштабирование на пиксели и ≤ 1 пикселя точности.
5. **Suspend:** запись PLS = U3 игнорируется; если Windows (selective suspend HID) будет ждать подтверждения, возможны таймауты. Не проверено.
6. **Supported Protocol без USB Legacy Support, ERST Max = 0, нулевой scratchpad, PSIC = 0** — приемлемость для `usbxhci` и прошивки не проверена.
7. **Редакция HID Usage Tables** не фиксирована; номера Usage (Keyboard/Keypad 0x07, LEDs 0x08, Generic Desktop 0x01, Button 0x09) — по памяти.
8. **Задержка сброса порта 10 мс** и `MFINDEX` от virtual clock — значения проектные.
9. **Reset-значения регистров и ответов при ошибочной записи** (раздел 4) — решение модели, не наблюдение за реальным контроллером.

## 15. Потребитель: прошивка (S7-34)

Минимальный срез S7-34 ([STAGE_7.md](../plans/STAGE_7.md#s7-34-usb-hid-клавиатура-прошивки-для-conin)), полностью определяемый этим контрактом. Ничего сверх списка в S7-34 не входит.

1. **Обнаружение.** Перечисление PCI по ECAM (handoff код 7): шина 0, устройства 0..31, функция 0; xHCI — первая функция с классом `0x0C0330`. Читает BAR0 (64 бита, запрограммирован платой; прошивка его не переназначает, проверяет, что размер `0x4000`), сохраняет Command, пишет Command = MSE | BME | INTx Disable. Функции нет — USB отсутствует, `ConIn` остаётся COM1, без ошибки и без сообщения на каждый опрос (одна диагностическая строка в COM1).
2. **Драйвер xHCI.** Чтение HCSPARAMS1/HCCPARAMS1 и проверка значений раздела 4 (CSZ = 0, MaxPorts ≥ 1); R/S = 0 → HCRST (ожидания — ограниченное число опросов, затем отказ без зависания); память: DCBAA, командное кольцо, кольцо событий (один сегмент) и ERST, Input/Output Contexts и кольца трансферов — в памяти `EfiBootServicesData`, выровненной по 64; CONFIG = 2 слота; DCBAAP, CRCR (RCS = 1), ERSTSZ = 1, ERSTBA, ERDP; USBCMD.R/S = 1; все ожидания — опросом, прерываний нет.
3. **Перечисление.** Для каждого порта с CCS = 1: очистить CSC, записать PR = 1, дождаться PRC = 1 и PED = 1, очистить PRC; Enable Slot; Address Device (BSR = 0, Speed = 1, EP0 Max Packet Size 64); GET_DESCRIPTOR device (18 байт) и configuration (34 байта, запрашивая сначала 9 байт, затем wTotalLength). Устройство подходит, если интерфейс имеет класс 3, subclass 1, protocol 1; иначе (указатель) слот освобождается (Disable Slot) и порт пропускается. Далее Configure Endpoint (добавить DCI 3, Context Entries = 3, тип 7, Max Packet Size 8, Interval 10) и SET_CONFIGURATION(1); порядок двух операций произвольный (раздел 10.1); SET_PROTOCOL(0) (boot), SET_IDLE(0), SET_REPORT(Output) с LED = 0. Только корневой хаб, без внешних хабов.
4. **HID → ConIn.** Interrupt IN TRB (8 байт, IOC = 1) ставится в кольцо, doorbell; опрос кольца событий на фазовый бит — по таймеру/при `ReadKeyStroke`/`WaitForKey`; после каждого события TRB переставляется. Отчёт переводится в `EFI_INPUT_KEY` (ScanCode/UnicodeChar) и `EFI_KEY_STATE` (RightShift/LeftShift, Ctrl, Alt, Logo; KeyToggleState — NumLock/CapsLock/ScrollLock с записью LED SET_REPORT) и публикуется через **`EFI_SIMPLE_TEXT_INPUT_PROTOCOL` и `EFI_SIMPLE_TEXT_INPUT_EX_PROTOCOL`**. Новая клавиша = Usage в массиве отчёта, которого не было в предыдущем; автоповтора в прошивке нет. Таблица Usage ID → клавиша — в `UEFI_PROFILE.md` («Консоль»).
5. **Объединение источников `ConIn`.** COM1 (S7-07) и USB пишут в одну ограниченную очередь клавиш (256), `WaitForKey` сигналит при непустой очереди; опрос — COM1, затем USB, в одном цикле. Нет USB — только COM1.
6. **Остановка при выходе из Boot Services.** Обработчик события группы выхода из Boot Services выполняет раздел 12.3 и восстанавливает Command.
7. **Проверка (S7-34).** Runner вставляет HID-события в очередь ввода; приложение `echo` печатает нажатые клавиши; после выхода из Boot Services значения регистров — раздел 12.3.

## 16. Потребители модели, тесты и API

**Очередь событий ввода машины** — ограниченная (256 событий), принадлежит `od-machine`: производитель (тестовый API runner; после S10-16 — host-путь из окна, [STAGE_10.md](../plans/STAGE_10.md)) вызывает `push_input(InputEvent)`, при переполнении получает `InputQueueFull`, событие не теряется молча. Модель только потребляет очередь в `service` и не вызывает host-API ввода. Для тестов — чтение LED клавиатуры, чтение счётчиков отбрасываний/слияния и счётчика обращений к RAM. Имена API ориентировочные, фиксирует S8-42.

**Обязательные тесты S8-42** (каждый отказ контракта — отдельный тест): значения всех регистров разделов 4–7 и ширины доступа; BAR, MSE, BME, INTx Disable; HCRST из любого состояния; Enable/Disable Slot (включая No Slots Available, Slot Not Enabled); Address Device (BSR 0/1, каждый Parameter/Context State Error, порт не сброшен); Configure Endpoint (добавление DCI 3, отказы, DC); Evaluate Context; No Op; неподдерживаемый тип (Stop/Reset Endpoint → TRB Error); Link TRB и перенос cycle; кольцо событий (wrap, Event Ring Full); Port Status Change (отложенное событие, повторный сброс порта); control GET_DESCRIPTOR device/configuration/string (в том числе 0xEE) с драйвером-тестом на Rust через гостевую RAM и MMIO; interrupt IN: отчёт после события ввода; все пути раздела 12 (HSE при DMA-ошибке, буфер вне RAM, остановка при выходе из Boot Services с проверкой отсутствия обращений к RAM).

**Обязательные тесты S8-43:** последовательности событий → байты отчётов (клавиатура: модификаторы, порядок, ErrorRollOver, повтор/лишний release; указатель: X/Y/кнопки/колесо, границы), байты дескрипторов и HID-запросы раздела 10, boot protocol клавиатуры по Appendix B, SET_PROTOCOL/IDLE/REPORT, переполнение очередей.

**S8-44 (Windows):** inbox `usbxhci`, `usbhub3`, `hidusb`/`kbdhid`/`mouhid` находят оба устройства; закрывает пункты раздела 14 (2–7).
