# Контракт xHCI и USB HID-устройств платы (v1)

**Дата:** 2026-11-20. **Задача:** [S8-41](../plans/STAGE_8.md#s8-41-описать-контракт-xhci-и-usb-hid). **Статус:** принято после архитектурного ревью; решение (а) подтверждено владельцем 2026-11-21; чекбокс S8-41 отмечен. Подготовлено архитектурным агентом (Claude Opus / `high`). Владелец модели — этап 8 по решению S8-01 (а): **принято архитектурным агентом с оговоркой, подтверждено владельцем 2026-11-21**. Модели в коде нет: реализацию дают S8-42 (контроллер) и S8-43 (HID-устройства). Потребители контракта — S7-34 (драйвер прошивки для `ConIn`), S8-44 (inbox-драйверы Windows), S10-16 (источник событий из окна).

Документ задаёт объём v1, значения регистров и поведение модели. Структуры памяти (контексты, TRB, Event Ring Segment Table) определяет спецификация; документ ссылается на её разделы и не пересказывает их побитно. Места, где модель выбирает поведение, а текст спецификации в этой задаче не сверен до конца, помечены **«сверить»**: S8-42 сверяет их с первоисточником до кода и записывает результат сюда. Значения, не найденные в первоисточнике, не угадываются.

## 1. Источники и редакции

Дата доступа — 2026-11-20.

| ID | Документ | Редакция | Получено |
|---|---|---|---|
| XHCI | Intel, [xHCI 1.2](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf) | Revision 1.2, May 2019 (обложка) | PDF 645 страниц, SHA-256 `9257ca24f34bff08e6020623f707c788ca0f0df657aa713309ff489a68b1731e`; текст — `pdftotext`. Прочитаны: таблицы 5-9…5-43 (регистры), §5.1 (соглашения), §5.2 (PCI), §7.2 и таблица 7-13 (Supported Protocol, скорости USB 2.0), таблица 6-90 (коды завершения), таблица 6-91 (типы TRB), таблица 6-94 (ERST), пометки «mandatory for xHCI 1.1/1.2» |
| USB2 | USB-IF, [USB 2.0 Specification](https://www.usb.org/document-library/usb-20-specification) | архив `usb_20_20250603.zip` (страница от 06/03/2025), файл `usb_20.pdf` | Глава 9: таблица 9-4 (коды стандартных запросов), таблица 9-5 (типы дескрипторов) |
| HID | USB-IF, [Device Class Definition for HID 1.11](https://www.usb.org/document-library/device-class-definition-hid-111) | Version 1.11, «Firmware Specification—5/27/01» | PDF SHA-256 `206272dacaf01ff34beda038b8ed59abbb02f028394896345c393bc026795d36`: §4.2–4.3 (subclass, protocol), §7.1–7.2 (дескрипторы и class-запросы), Appendix B (boot-отчёты) |
| HUT | USB-IF, [HID Usage Tables 1.6](https://usb.org/sites/default/files/hut1_6.pdf) | Version 1.6 | PDF SHA-256 `c2fbeeacdfdc813cae684d768aa5baf0a29b37216befca3b9218294d25f72c26`: страницы Generic Desktop (`0x01`), Keyboard/Keypad (`0x07`), LED (`0x08`), Button (`0x09`) |
| MS-TLC | Microsoft, [Top-level collections opened by Windows for system use](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/top-level-collections-opened-by-windows-for-system-use) | `ms.date` 2024-01-11 | Generic Desktop `0x01`/`0x02` — «Mouse», `0x01`/`0x06` — «Keyboard», доступ «Exclusive» |
| MS-KBM | Microsoft, [Keyboard and mouse HID client drivers](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/keyboard-and-mouse-hid-client-drivers) | `ms.date` 2025-04-16 | `MOUHID.sys` переводит HID-usages в команды мыши; «MOUCLASS.sys … supports both absolute and relative pointing devices» |
| MS-USB | Microsoft, [USB driver stack architecture](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/usb-3-0-driver-stack-architecture) | `ms.date` 2024-01-17 | Windows загружает USB 3.0 стек (`Usbxhci.sys`), когда устройство подключено к xHCI |

**Источник не найден:** какой минимальный HCIVERSION и какие из возможностей xHCI 1.1/1.2 требует inbox `usbxhci`; использует ли он MSI-X, MSI или INTx; как `mouhid` обрабатывает отчёт с абсолютными X/Y в коллекции Mouse. Всё это проверяется на S8-44 (раздел 9).

## 2. Объём v1

- Один PCI-контроллер xHCI, class code `0C0330` (XHCI §5.2.2: PI `30h`, SCC `03h`, BASEC `0Ch`).
- Интерфейс **xHCI 1.0** (`HCIVERSION = 0100h`) по тексту редакции 1.2. Основание: XHCI объявляет обязательными для «xHCI 1.1 and xHCI 1.2 compliant xHCs» Stopped EDTLA (SEC), Stopped – Short Packet (SPC), Contiguous Frame ID (CFC), U3 Entry (U3C), Force Save Context (FSC), Hardware LPM и BESL LPM для USB2 (HLC, BLC). Для двух HID-устройств они не нужны, а объявление версии без них было бы ложным. Альтернатива — `0120h` с полным набором этих функций; выбор пересматривается, если S8-44 покажет, что `usbxhci` требует 1.1+.
- Корневой хаб: **2 порта USB 2.0**, без внешних хабов. Порт 1 — клавиатура, порт 2 — абсолютный указатель; оба устройства full-speed и подключены всё время жизни машины.
- Один Interrupter (IR0); MSI-X с одним вектором и INTx.
- Command Ring, один Event Ring (до 16 сегментов), Transfer Rings для EP0 (control) и одного interrupt IN endpoint на устройство.
- Команды: No Op, Enable Slot, Disable Slot, Address Device, Configure Endpoint, Evaluate Context, Reset Endpoint, Stop Endpoint, Set TR Dequeue Pointer, Reset Device. Disable Slot и Reset Device добавлены к перечню плана: таблица 6-91 не помечает их как optional, а драйвер ОС использует их при сбросе и удалении устройства (наблюдать на S8-44).
- Передачи: control (Setup/Data/Status Stage) и interrupt IN (Normal TRB).
- HID boot keyboard и абсолютный указатель с собственными дескрипторами (раздел 7).

## 3. PCI-функция и размещение

| Поле | Значение | Примечание |
|---|---|---|
| Vendor ID / Device ID | `0xD1A6` / `0x0009` (предложение) | Тестовый VID, как у NVMe-endpoint (`0xD1A6`/`0x0008`); поставочный VID — решение владельца, как для NVMe. Драйверы привязываются по class code |
| Revision ID | `0x00` | — |
| Class code | `0x0C0330` | XHCI §5.2.2 |
| Header type | Type 0 | XHCI §5.2.1 |
| BAR0/BAR1 | 64-битный non-prefetchable memory BAR, **64 KiB** | XHCI §5.2.1: BAR0 и BAR1 обязательны для 64-битной адресации |
| Capabilities pointer | `0x50` → MSI-X | Других capabilities в v1 нет (PCI Power Management — раздел 10) |
| MSI-X | 1 вектор; таблица — `BAR0 + 0x3000`, PBA — `BAR0 + 0x4000` | Семантика Enable, Function Mask, Vector Mask и PBA — как у NVMe ([NVME_DEVICE.md](NVME_DEVICE.md) §3.1) |
| Interrupt Pin | INTA | Маршрутизация в GSI и запись `_PRT` — S8-42 |
| SBRN (`0x60`) | `0x30` | XHCI таблица 5-5 допускает только `30h`/`31h`/`32h`; значение относится к контроллеру, порты v1 — только USB 2.0 |
| FLADJ (`0x61`) | `0x20`, RWS, хранится без эффекта | XHCI §5.2.4: значение по умолчанию `20h` |
| DBESL/DBESLD (`0x62`) | `0x00`, RO | LPM в v1 нет |

**Адрес BAR и номер функции** назначает S8-42 при регистрации в плате вместе с S8-20 (дополнительные NVMe) по карте [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §2. Предложение: функция `00:08.0`, BAR0 = `0xFC20_0000` в свободном участке выше ECAM `[0xFC10_0000, 0xFC20_0000)` и ниже IOAPIC `0xFEC0_0000`; пересечения проверяет плата. Запись handoff для прошивки (S7-34) и описание в ACPI — S8-42 по согласованию с владельцем `FIRMWARE_BOOT.md` (этап 7). MMIO декодируется при PCI Command.MSE и адресе BAR внутри апертуры, DMA — при BME (как NVMe).

## 4. Регистры

**Ширина доступа.** Все регистры, кроме двух первых полей capability, — Dword по выровненному адресу. 64-битные регистры (`CRCR`, `DCBAAP`, `ERSTBA`, `ERDP`) — Qword по выровненному адресу или два Dword: сначала младший, затем старший (XHCI §5.1, при `AC64 = 1`). `CAPLENGTH` дополнительно читается байтом по `0x00`, `HCIVERSION` — словом по `0x02` (выбор модели: XHCI задаёт размер этих полей 8 и 16 бит). Другая ширина или невыровненный адрес — ошибка шины `UnsupportedWidth`/`Misaligned` по соглашению [NVME_DEVICE.md](NVME_DEVICE.md) §4. Записи в RO-поля и поля Rsvd игнорируются (не ошибка шины): драйверы пишут регистр целиком. Чтение неописанных смещений внутри BAR0 даёт 0, запись игнорируется. Многобайтовые поля — little-endian (XHCI §5.1).

**Атрибуты** — по XHCI таблица 5-3: RO, RW, RW1C, RW1S, ROS, RWS, RW1CS, RsvdP, RsvdZ.

### 4.1. Capability registers (`BAR0 + 0x0000`)

| Смещение | Регистр | Ширина | Доступ | Значение | Источник поля |
|---|---|---|---|---|---|
| `0x00` | `CAPLENGTH` | 8 | RO | `0x20` — операционные регистры с `0x20` | XHCI таблица 5-9, §5.3.1 |
| `0x01` | Rsvd | 8 | — | 0 | таблица 5-9 |
| `0x02` | `HCIVERSION` | 16 | RO | `0x0100` | §5.3.2 (BCD, «0100h corresponds to xHCI version 1.0.0») |
| `0x04` | `HCSPARAMS1` | 32 | RO | `0x0200_0108`: MaxSlots [7:0] = 8, MaxIntrs [18:8] = 1, MaxPorts [31:24] = 2 | таблица 5-10 |
| `0x08` | `HCSPARAMS2` | 32 | RO | `0x0000_0040`: IST [3:0] = 0, ERST Max [7:4] = 4 (до 2⁴ = 16 сегментов), Max Scratchpad Bufs Hi/Lo = 0, SPR = 0 | таблица 5-11 |
| `0x0C` | `HCSPARAMS3` | 32 | RO | `0` (U1/U2 Device Exit Latency — только USB3) | таблица 5-12 |
| `0x10` | `HCCPARAMS1` | 32 | RO | `0x0200_0081`: AC64 [0] = 1, BNC = 0, CSZ [2] = 0 (контексты 32 байта), PPC [3] = 0, PIND = 0, LHRC = 0, LTC = 0, NSS [7] = 1, PAE = 0, SPC = 0, SEC = 0, CFC = 0, MaxPSASize [15:12] = 0, xECP [31:16] = `0x200` (Dword-смещение `0x800`) | таблица 5-13 |
| `0x14` | `DBOFF` | 32 | RO | `0x0000_2000` (биты [31:2], Dword-выравнивание) | §5.3.7 |
| `0x18` | `RTSOFF` | 32 | RO | `0x0000_1000` (биты [31:5], выравнивание 32 байта) | §5.3.8 |
| `0x1C` | `HCCPARAMS2` | 32 | RO | `0`: U3C, CMC, FSC, CTC, LEC, CIC, ETC, ETC_TSC, GSC, VTC не объявлены | таблица 5-16 |

### 4.2. Операционные регистры (`BAR0 + 0x0020`)

| Смещение от `0x20` | Регистр | Ширина | Доступ | Значение после создания и HCRST | Поведение модели |
|---|---|---|---|---|---|
| `0x00` | `USBCMD` | 32 | RW по полям | `0` | R/S [0]: 0→1 при `HCH = 1` запускает контроллер (`HCH = 0`); 1→0 останавливает обработку колец, `HCH = 1` к следующему чтению. HCRST [1]: запись 1 выполняет сброс синхронно (раздел 8), затем бит читается как 0. INTE [2], HSEE [3] — RW. LHCRST [7] — RO 0 (`LHRC = 0`). CSS [8], CRS [9]: Save/Restore не поддержаны — запись 1 ставит `USBSTS.SRE = 1` (**сверить** с §4.23.2). EWE [10], EU3S [11], CME [13], ETE [14], TSC_EN [15], VTIOE [16] — хранятся как RW, эффекта нет (функции не объявлены); RsvdP сохраняются |
| `0x04` | `USBSTS` | 32 | RO/RW1C | `0x0000_0001` (HCH = 1, CNR = 0) | HCH [0] RO; HSE [2] RW1C; EINT [3] RW1C; PCD [4] RW1C; SSS [8], RSS [9] RO 0; SRE [10] RW1C; CNR [11] RO: **0** — модель готова сразу после создания и HCRST (XHCI задаёт значение по умолчанию 1, пока контроллер не готов; модель становится готовой синхронно); HCE [12] RO 0 |
| `0x08` | `PAGESIZE` | 32 | RO | `0x0000_0001` (4 KiB) | §5.4.3: «if bit 0 is Set, the xHC supports 4k byte page sizes» |
| `0x14` | `DNCTRL` | 32 | RW | `0` | Хранится; Device Notification не генерируются (раздел 10) |
| `0x18` | `CRCR` | 64 | RW/RW1S/RO | `0` | RCS [0], указатель [63:6]: запись игнорируется при `CRR = 1`. CS [1] и CA [2] (RW1S): останавливают/прерывают Command Ring с событием Command Ring Stopped (код 24) или Command Aborted (код 25) по §4.6.1.1–4.6.1.2 (**сверить**). CRR [3] RO. Чтение поля указателя всегда даёт 0 (таблица 5-24: «Reading this field always returns ‘0’») |
| `0x30` | `DCBAAP` | 64 | RW | `0` | [5:0] RsvdZ (выравнивание 64 байта); массив на `MaxSlotsEn + 1` записей, запись 0 (scratchpad) не используется, так как Max Scratchpad Bufs = 0 |
| `0x38` | `CONFIG` | 32 | RW | `0` | MaxSlotsEn [7:0]: 0…8, запись больше 8 — значение не меняется (**сверить** с §5.4.7); пишется только при `HCH = 1`. U3E [8], CIE [9] — хранятся (U3C = 0, CIC = 0) |
| `0x400 + 0x10·(n−1)`, n = 1, 2 | `PORTSC` | 32 | по полям | раздел 6 | раздел 6 |
| `+0x4` | `PORTPMSC` | 32 | RWS/RsvdP | `0` | USB2-формат таблицы 5-30; LPM не поддержан: L1 Status RO 0, остальные поля хранятся без эффекта |
| `+0x8` | `PORTLI` | 32 | RO | `0` | для USB2-портов reserved |
| `+0xC` | `PORTHLPMC` | 32 | RsvdP | `0` | `HLC = 0` |

### 4.3. Runtime registers (`BAR0 + 0x1000`)

| Смещение от `0x1000` | Регистр | Ширина | Доступ | Значение | Поведение модели |
|---|---|---|---|---|---|
| `0x00` | `MFINDEX` | 32 | RO | `0` | [13:0] — номер микрокадра; при `R/S = 1` растёт на 1 каждые 125 мкс virtual clock этапа 4, при остановке не меняется. MFINDEX Wrap Event не генерируется |
| `0x20` | `IMAN` (IR0) | 32 | RW1C/RW | `0` | IP [0] RW1C, IE [1] RW (таблица 5-38) |
| `0x24` | `IMOD` | 32 | RW | IMODI [15:0] = 4000 (таблица 5-39: «Default = ‘4000’ (~1ms)»), IMODC = 0 | Значение хранится; **интервал модерации в v1 не применяется** — прерывание формируется без задержки (раздел 10) |
| `0x28` | `ERSTSZ` | 32 | RW | `0` | [15:0] — число записей ERST, допустимо 1…16 (`2^ERST Max`) |
| `0x30` | `ERSTBA` | 64 | RW | `0` | [63:6]; запись указателя (старшего Dword или Qword) включает Event Ring и читает ERST (§4.9.4, **сверить** порядок с ERSTSZ) |
| `0x38` | `ERDP` | 64 | RW/RW1C | `0` | DESI [2:0] RW, EHB [3] RW1C, указатель [63:4] RW |

Interrupter Register Sets с номером больше 0 не существуют (`MaxIntrs = 1`): чтение 0, запись игнорируется.

### 4.4. Doorbell Array (`BAR0 + 0x2000`)

Регистры `DB0`…`DB8` по 4 байта (`MaxSlots = 8`). DB Target [7:0], DB Stream ID [31:16] (таблица 5-43); при чтении 0. `DB0`: Target 0 — Command Doorbell; другие значения — игнорируются. `DB1`…`DB8`: Target 1 — EP0, 2…31 — `EPn OUT/IN` (Device Context Index). Stream ID ≠ 0 при `MaxPStreams = 0` — doorbell игнорируется (таблица 5-43). Doorbell неактивного слота или эндпоинта игнорируется. Запись doorbell только отмечает работу; исполнение — в `service` (раздел 5).

### 4.5. Extended capabilities (`BAR0 + 0x0800`)

Единственная запись — **xHCI Supported Protocol Capability** для USB 2.0 (XHCI §7.2, таблицы 7-6…7-9): Dword 0 = `0x0200_0002` (Capability ID 2, Next = 0, Minor Revision `00h`, Major Revision `02h`); Dword 1 = `0x2042_5355` (Name String «USB »); Dword 2 = `0x0000_0201` (Compatible Port Offset 1, Compatible Port Count 2, Protocol Defined = 0, PSIC = 0 — скорости по умолчанию таблицы 7-13: 1 = Full-speed, 2 = Low-speed, 3 = High-speed); Dword 3 = 0 (Protocol Slot Type 0). USB Legacy Support Capability (ID 1) не объявляется: передачу владения от прошивки к ОС v1 решает остановкой контроллера (раздел 8).

## 5. Модель обслуживания и DMA

- Модель следует схеме NVMe ([NVME_DEVICE.md](NVME_DEVICE.md) §3): `BusDevice::read/write` не обращаются к RAM гостя; doorbell и запись `PORTSC.PR` ставят флаг работы, а кольца, контексты и буферы обрабатываются в `service(&mut MachineBus, committed_write)` между квантами гостя. Порт-сброс, команды и передачи завершаются в ближайшем `service`; время между doorbell и событием virtual clock не моделирует.
- DMA — только в RAM гостя по контракту машины ([MACHINE_CONTRACT.md](MACHINE_CONTRACT.md)); записи сообщаются в `committed_write`. Адрес вне RAM (MMIO, ROM, framebuffer, вне карты, выше физической ширины 36 бит), чтение кольца или контекста с ошибкой — **Host System Error**: `USBSTS.HSE = 1`, обработка колец прекращается, `HCH = 1`; выход — HCRST (**сверить** с описанием HSE в таблице 5-21).
- Структуры: Device Context Base Address Array (§6.1), Device/Slot/Endpoint/Input Context размером 32 байта на контекст (`CSZ = 0`, §6.2), TRB 16 байт с Cycle bit и полем типа (§4.11, §6.4; позиции полей сверяет S8-42), Event Ring Segment Table: запись с базой сегмента и размером 16…4096 TRB (таблица 6-94). Cycle-состояние колец и Link TRB с Toggle Cycle — §4.9.
- **Command Ring:** исполняются только типы из раздела 2. Тип, не разрешённый для Command Ring, или нереализованная optional-команда (Force Event, Negotiate Bandwidth, Set LTV, Get Port Bandwidth, Get/Set Extended Property) и Force Header → Command Completion Event с кодом TRB Error (5), указателем на TRB и Slot ID 0 (примечание к таблице 6-91; для Force Header на USB2-порту — **сверить** §4.6.16).
- **Event Ring:** модель не перезаписывает непрочитанные события. Если для события нет места, модель ставит обработку команд и передач на паузу до продвижения `ERDP`; событие Event Ring Full Error (код 21) формируется по §4.9.4 (**сверить**). Внутренняя очередь событий ограничена; её переполнение невозможно, пока обработка стоит на паузе.
- **Прерывание:** после записи события при `IMAN.IE = 1` и `USBCMD.INTE = 1`, если `ERDP.EHB = 0`: `IMAN.IP = 1`, `ERDP.EHB = 1`, `USBSTS.EINT = 1`; затем MSI-X-вектор 0 (при MSI-X Enable) или удержание INTA, пока `IMAN.IP = 1` (поведение INTA — **сверить** с §4.17). MSI-X: Fixed edge-доставка в существующий xAPIC, маски и PBA — как NVMe.

## 6. Корневой хаб и порты

Порты 1 и 2 — USB 2.0 (§4.5). `PPC = 0`: `PORTSC.PP = 1` всегда, запись PP игнорируется. Устройства подключены с момента создания машины.

| Поле `PORTSC` | Значение и поведение |
|---|---|
| CCS [0] ROS | 1 (устройство подключено) |
| PED [1] RW1CS | 0 после создания и HCRST; 1 после завершения Port Reset; запись 1 выключает порт (PED = 0, PEC не ставится при программном выключении — **сверить** таблицу 5-27) |
| OCA [3] RO | 0 |
| PR [4] RW1S | запись 1 начинает сброс порта; в ближайшем `service`: PR = 0, PRC = 1, PED = 1, PLS = U0 (0), Port Speed = 1 (Full-speed), устройство — в состоянии Default (адрес 0) |
| PLS [8:5] RWS | до сброса — состояние подключённого, но не включённого USB2-порта по §4.19.1.2 (значение Polling/Disabled — **сверить** с диаграммой USB2 port state machine); после сброса U0. Запись с LWS = 1 и PLS = U3/U0 (suspend/resume) — v1 принимает U3 и возврат в U0 без remote wakeup (**сверить** §4.19.1.2), остальные значения игнорируются |
| PP [9] RWS | 1 |
| Port Speed [13:10] ROS | 0 до первого Port Reset («This field is invalid on a USB2 protocol port until after the port is reset», таблица 5-27), затем 1 |
| PIC [15:14] | 0 (`PIND = 0`) |
| LWS [16] RW | строб записи PLS, читается 0 |
| CSC [17], PEC [18], PRC [21], PLC [22] RW1CS | изменения; CSC = 1 после создания машины и после HCRST |
| WRC [19], CEC [23] | RsvdZ на USB2-портах |
| OCC [20] RW1CS | 0 |
| CAS [24] RO | 0 |
| WCE, WDE, WOE [25:27] RWS | хранятся, эффекта нет |
| DR [30] RO | 0 (устройства несъёмные) |
| WPR [31] | RsvdZ на USB2-портах |

**Port Status Change Event** (тип 34) ставится в Event Ring при переходе любого из CSC, PEC, PRC, PLC из 0 в 1 и `USBSTS.PCD = 1` (§4.19.2, **сверить** условия повторной генерации, пока бит не сброшен).

## 7. Устройства USB HID

Общие для обоих устройств значения (USB2 глава 9, HID §7). Все дескрипторы — собственные, составлены по полям USB2 и HID; байтовую кодировку items HID §6.2.2 составляет S8-43 и проверяет собственным разбором в тесте. Чужие дескрипторы (QEMU, Bochs, VirtualBox, реальные устройства) не копируются.

| Поле | Клавиатура (порт 1) | Указатель (порт 2) |
|---|---|---|
| Скорость | Full-speed | Full-speed |
| Device: `bcdUSB`, class/subclass/protocol | `0x0200`, 0/0/0 (класс на интерфейсе) | то же |
| `bMaxPacketSize0` | 64 | 64 |
| `idVendor` / `idProduct` / `bcdDevice` | `0xD1A6` / `0x0001` / `0x0100` — тестовые; поставочный USB VID — решение владельца | `0xD1A6` / `0x0002` / `0x0100` |
| Строки (язык `0x0409`) | «Open Desktop», «Open Desktop Keyboard» | «Open Desktop», «Open Desktop Pointer» |
| Конфигурации | 1, `bmAttributes` = bus-powered без remote wakeup, `bMaxPower` — 100 мА | то же |
| Интерфейс | class 3 (HID), subclass 1 (Boot), protocol 1 (Keyboard) — HID §4.2–4.3 | class 3, subclass 0, protocol 0: boot-протокол мыши относительный (HID Appendix B.2), абсолютному указателю он не подходит |
| HID descriptor (`0x21`) | `bcdHID = 0x0111`, `bCountryCode = 0`, один дескриптор отчёта (`0x22`) | то же |
| Endpoint | `0x81`, interrupt IN, `wMaxPacketSize = 8`, `bInterval = 8` (мс) | `0x81`, interrupt IN, `wMaxPacketSize = 8`, `bInterval = 8` |

**Стандартные запросы** (USB2 таблица 9-4): GET_STATUS (0), CLEAR_FEATURE (1) и SET_FEATURE (3) для ENDPOINT_HALT, SET_ADDRESS (5), GET_DESCRIPTOR (6: DEVICE, CONFIGURATION, STRING, а также HID и REPORT через интерфейс), GET_CONFIGURATION (8), SET_CONFIGURATION (9: 0 или 1), GET_INTERFACE (10) и SET_INTERFACE (11: только alt 0). Остальные запросы, SET_DESCRIPTOR (7), DEVICE_QUALIFIER и OTHER_SPEED_CONFIGURATION (типы 6, 7 — у full-speed устройства их нет) — STALL → код завершения Stall Error (6).

**Class-запросы HID** (HID §7.2): GET_REPORT (`0x01`: Input — текущий отчёт; Output у клавиатуры — состояние LED), GET_IDLE (`0x02`), GET_PROTOCOL (`0x03`, только клавиатура), SET_REPORT (`0x09`, Output у клавиатуры — LED), SET_IDLE (`0x0A`), SET_PROTOCOL (`0x0B`, только клавиатура: 0 — boot, 1 — report). После сброса устройства — report protocol («When initialized, all devices default to report protocol», HID §7.2.6). Idle по умолчанию: клавиатура — 500 мс, указатель — бесконечность (HID §7.2.4: «recommended default idle rate … is 500 milliseconds for keyboards … and infinity for joysticks and mice»). При idle ≠ 0 устройство повторяет последний отчёт по истечении idle-времени virtual clock даже без изменений (HID: клавиатура «must send data reports at the Idle rate»).

### 7.1. Клавиатура

Отчёт ввода — 8 байт, одинаковый в boot и report protocol (HID Appendix B.1): байт 0 — модификаторы (биты 0…7 = usages `0xE0`…`0xE7` страницы Keyboard/Keypad), байт 1 — зарезервирован (0), байты 2…7 — до шести usages нажатых клавиш. Если нажато больше шести клавиш, все шесть позиций содержат ErrorRollOver (`0x01`, HUT; HID: «must report a phantom state indexing Usage(ErrorRollOver)»). Отчёт вывода — 1 байт: биты 0…4 — LED Num Lock, Caps Lock, Scroll Lock, Compose, Kana (страница LED), биты 5…7 — 0.

Собственный дескриптор отчёта описывает ровно этот формат: Application Collection Generic Desktop / Keyboard (`0x01`/`0x06`); 8 однобитных полей Input (Data, Variable, Absolute) для usages `0xE0`…`0xE7`; одно 8-битное поле Input (Constant); 5 однобитных полей Output (Data, Variable, Absolute) страницы LED, usages 1…5; 3 бита Output (Constant); 6 восьмибитных полей Input (Data, Array) страницы Keyboard/Keypad с Logical Minimum 0, Logical Maximum `0xE7`, Usage Minimum 0, Usage Maximum `0xE7`. Report ID не используется. **Предупреждение для S8-43:** `0xE7` = 231 не помещается в знаковый однобайтовый item — `0x25 0xE7` при знаковом чтении даёт −25, и верхняя граница окажется меньше нижней; Logical Maximum кодируется двухбайтовым item `0x26 0xE7 0x00` (little-endian), и тест S8-43 разбирает дескриптор независимым разбором с проверкой знака. Правило знаковости Logical Minimum/Maximum и размеров item — HID 1.11 §6.2.2.7, **сверить** (в этой правке по PDF не сверено); то же — для Usage Maximum `0xE7`.

### 7.2. Абсолютный указатель

Отчёт ввода — 5 байт: байт 0 — кнопки 1…3 (биты 0…2, страница Button), биты 3…7 — 0; байты 1–2 — X, байты 3–4 — Y, 16 бит little-endian, диапазон 0…32767. Собственный дескриптор: Application Collection Generic Desktop / Mouse (`0x01`/`0x02`) — эту коллекцию Windows открывает как «Mouse» (MS-TLC), внутри Physical Collection Pointer (`0x01`); 3 однобитных поля Input (Data, Variable, Absolute) Button 1…3; 5 бит Input (Constant); X (`0x30`) и Y (`0x31`) — по одному 16-битному полю Input (Data, Variable, **Absolute**), Logical Minimum 0, Logical Maximum 32767. Report ID не используется; колеса нет (раздел 10).

Что `mouhid` переводит такие отчёты в абсолютные перемещения, подтверждено только косвенно (MS-KBM: `MOUCLASS` поддерживает абсолютные устройства; поведение `MOUHID` для абсолютных X/Y — **источник не найден**). Проверка — S8-44 (точность ≤ 1 гостевого пикселя). Если не подтвердится, запасной вариант — коллекция Digitizer (страница `0x0D`) по документации Microsoft; это меняет контракт и требует нового ревью.

### 7.3. Источник событий и время

- Модель принимает события через API машины (не host-API ввода): `Key { usage: u8 /* страница 0x07 */, pressed: bool }`, `PointerAbsolute { x: u16, y: u16 }` в нормированных координатах 0…32767, `PointerButton { button: 1..=3, pressed: bool }`. Перевод координат окна в нормированные — сторона host (S10-16); до S10-16 — тестовый API runner. Точные имена типов задаёт S8-43.
- События применяются в `service` в порядке поступления. Каждое событие клавиши даёт отдельный отчёт (нажатия не сливаются); последовательные перемещения указателя без промежуточной выборки гостем сливаются в последнее положение; изменения кнопок не сливаются.
- Очередь отчётов на устройство ограничена (64 отчёта клавиатуры, 64 указателя). При заполнении новое событие отклоняется вызывающей стороне host с ошибкой; гостевое состояние не меняется.
- Interrupt IN TD завершается, только когда есть новый отчёт или истёк idle-таймер; иначе TD ждёт (эквивалент NAK). Время — virtual clock этапа 4.

## 8. Ошибки, сброс и передача от прошивки ОС

**Гостевые ошибки** (код завершения в Event TRB, таблица 6-90; ни одна не останавливает машину):

| Условие | Код |
|---|---|
| Успех | Success (1) |
| Неверный тип TRB, неверное поле TRB, нереализованная optional-команда | TRB Error (5) |
| STALL устройства на control-запрос | Stall Error (6) |
| Configure Endpoint/Address Device: эндпоинт типа isoch или bulk, больше эндпоинтов, чем модель поддерживает | Resource Error (7) |
| Enable Slot при занятых `MaxSlotsEn` слотах | No Slots Available Error (9) |
| Команда для слота в состоянии Disabled | Slot Not Enabled Error (11) |
| Doorbell/команда для эндпоинта в состоянии Disabled | Endpoint Not Enabled Error (12) |
| Принято меньше байт, чем в TD | Short Packet (13) |
| Неверный параметр контекста | Parameter Error (17) |
| Переход из недопустимого состояния контекста | Context State Error (19) |
| Нет места в Event Ring | Event Ring Full Error (21) — по §4.9.4 (**сверить**) |
| Stop/Abort Command Ring | Command Ring Stopped (24), Command Aborted (25) |
| Stop Endpoint во время передачи | Stopped (26) |
| Передача эндпоинту, которого нет у устройства | USB Transaction Error (4) |

Коды Data Buffer Error, Babble, Bandwidth, Ring Underrun/Overrun, Missed Service, Isoch и Stream-коды в v1 не формируются (нет физической шины, isoch и streams). Применимость каждого кода к конкретной команде сверяется S8-42 по описанию команды (§4.6.x) до кода.

**Ошибки DMA** — Host System Error (раздел 5). **Ошибки host/backend** (переполнение очереди событий, нарушение внутреннего инварианта модели) не являются гостевыми: первая возвращается вызывающей стороне host, вторая — ошибкой устройства машине без panic (как в NVMe, [NVME_DEVICE.md](NVME_DEVICE.md) §14.3); модель не маскирует её гостевым кодом.

**HCRST** (`USBCMD.HCRST = 1`): синхронный сброс. Операционные и runtime-регистры, `PORTSC`/`PORTPMSC` (XHCI: «only reset by platform hardware during a cold reset or in response to a Host Controller Reset») возвращаются к значениям разделов 4 и 6; все слоты Disabled, сведения о кольцах и `ERST` забыты, `MFINDEX = 0`; устройства — в состоянии Default (адрес 0, не сконфигурированы, report protocol, idle по умолчанию, LED выключены, очереди отчётов пусты); порты — CCS = 1, CSC = 1, PED = 0. PCI-конфигурация, состояние MSI-X и регистры capability не меняются. После сброса `HCRST = 0`, `CNR = 0`, `HCH = 1`.

**ExitBootServices.** Драйвер прошивки S7-34 перед выходом из Boot Services останавливает контроллер: `R/S = 0`, ожидание `HCH = 1` (UEFI 2.11 «USB Host Controller Protocol» и XHCI — по формулировке S7-34). Модель после этого не исполняет кольца и не пишет в RAM. Предположение, что ОС начинает с HCRST (и поэтому указатели прошивки после сброса не используются), — **источник не найден**, наблюдать на S8-44 (раздел 9, п. 5); если драйвер не сбросит контроллер, модель всё равно остаётся остановленной (`R/S = 0`) до команды драйвера. Без USB Legacy Support Capability согласования владения нет.

## 9. Что проверяется на S8-44 (источник не найден)

1. Принимает ли `usbxhci` контроллер с `HCIVERSION = 0100h` и без функций xHCI 1.1/1.2 (Device Manager без ошибок).
2. Использует ли `usbxhci` MSI-X с одним вектором, MSI или INTx (журнал модели).
3. Переводит ли `mouhid` отчёт раздела 7.2 в абсолютные координаты (точность ≤ 1 пикселя).
4. Какие команды Command Ring выдаёт драйвер (журнал модели), в том числе Disable Slot и Reset Device.
5. Начинает ли `usbxhci` работу с `USBCMD.HCRST = 1` после передачи от прошивки S7-34 (журнал модели: порядок записей в `USBCMD`); от ответа зависит, надо ли модели что-либо делать с кольцами и `ERST`, оставленными прошивкой.

## 10. Не входит в v1

- USB 3.x SuperSpeed порты и протокол, USB3-регистры портов, U1/U2/LTM, Force Header по существу.
- Streams, isochronous и bulk эндпоинты, mass storage, внешние хабы (`Hub = 1` в Slot Context → Parameter Error), USB passthrough.
- Несколько Interrupter, модерация прерываний по `IMOD`, MFINDEX Wrap Event, Device Notification, Bandwidth Negotiation, Light HC Reset, Save/Restore state, Hardware LPM и BESL, Port Power switching, Port Indicators, remote wakeup, VTIO, Debug Capability, USB Legacy Support Capability, PCI Power Management capability и D3.
- Колесо и кнопки 4–5 указателя, interrupt OUT для LED, несколько конфигураций и alternate settings, строки на языках кроме `0x0409`, горячее подключение и отключение устройств.
- Сохранение состояния контроллера и устройств в снимках VM — этап 9.

## 11. Проверка контракта (S8-42, S8-43)

- Host-side тест-драйвер на Rust через гостевую RAM и MMIO: сброс контроллера, настройка Command Ring/Event Ring, Port Reset, Enable Slot, Address Device, GET_DESCRIPTOR, SET_CONFIGURATION, Configure Endpoint, получение interrupt-отчёта; каждая строка таблицы ошибок раздела 8 — отдельный тест.
- Тест дескрипторов: собственный разбор items HID §6.2.2 восстанавливает форматы отчётов разделов 7.1 и 7.2; boot-отчёт клавиатуры совпадает с HID Appendix B.1 по раскладке байтов.
- Регистры: значения после создания и HCRST, RO/RW1C/RW1S-поведение, ширина доступа и `UnsupportedWidth`/`Misaligned`.
