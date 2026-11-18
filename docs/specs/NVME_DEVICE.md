# Минимальный NVMe-контроллер (S7-16)

**Статус S7-16: в работе.** Срез A реализует регистры, admin-очереди и Admin-команды. Срез B добавляет `Read`, `Write`, `Flush` поверх backend диска. Для firmware используется фиксированное MMIO с опросом. Для Linux direct boot доступен opt-in PCI endpoint с INTx и пятью векторами MSI-X. Постоянный RAW backend реализован в `od-host` (раздел 14). Добавлены страницы журналов `0x00`, `0x05` и `0x12`. Открыты проверка драйвера в запущенном госте и оставшаяся сверка по разделу 11.

Код: `crates/od-machine/src/nvme/{mod,controller,queues,admin,io,backend}.rs`, интеграция в плату — `crates/od-machine/src/firmware_board.rs`. Тесты — `nvme/tests.rs`, `nvme/queues.rs` (PRP), `firmware_board.rs` (handoff и `run_boot`).

## 1. Источники и статус сверки

Редакция по плану: NVM Express Base Specification 2.4, NVM Command Set Specification 1.3, NVMe over PCIe Transport Specification 1.4 (ratified 2026-07-31, [VIRTUAL_BOARD.md](VIRTUAL_BOARD.md)).

На 2026-11-18 получены [NVMe Base 2.4](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf) и [PCIe Transport 1.4](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVMe-over-PCIe-Transport-Specification-Revision-1.4-Ratified-2026.07.31.pdf). Figure 31 Base 2.4 требует от I/O controller страницы `0x00`, `0x05` и `0x12`. Их форматы и объявление поддержки в Identify Controller сверены по разделу 9.2. §5.3.4 допускает явное и неявное завершение команд удаляемой SQ. §3.5.1 и §3.8.1 PCIe Transport описывают INTx и BAR; §3.8.4 оставляет MSI-X опциональным в стандарте, а план S7-16 включает MSI-X в объём проекта. Полная сверка контроллера остаётся открыта.

Перед включением контроллера спецификация требует допустимые значения `CC.AMS`, `CC.MPS` и `CC.CSS`. Figure 41 связывает допустимые `CC.IOSQES` и `CC.IOCQES` с SQES/CQES в Identify Controller. Таблицы статусов Create I/O CQ и Create I/O SQ относят `Invalid Queue Size` к неинициализированному `CC.IOCQES` и `CC.IOSQES` соответственно. Модель проверяет соответствующее поле при обработке каждой команды. [NVMe Base 2.4, с. 59–60](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=85), [Figure 574](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=554), [Figure 579](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=557), [ISSUE-519](../KNOWN_ISSUES.md#issue-519).

`CAP.CQR` сверено по Figure 36. Бит 16 при значении 1 требует физически непрерывных очередей и PC в Create I/O SQ/CQ. В модели выставлен бит 16, а `queue_geometry` проверяет PC. [NVMe Base 2.4, с. 58](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=84)

При первой сверке 2026-11-07 использовался заголовок Linux `include/linux/nvme.h` (master):

- страница `https://nvmexpress.org/specifications/` открывается, но прямых ссылок на PDF не содержит (загрузка идёт через форму);
- PDF Base 2.4 по угаданным адресам не найден (HTTP 404); в архиве (`nvm-express-specification-archives/`) последняя найденная редакция — Base 2.3 (`NVM-Express-Base-Specification-Revision-2.3-2025.08.01-Ratified.pdf`);
- `WebFetch` для этого PDF отказал (размер больше 10 MiB), индексатор вернул сырые сжатые потоки PDF без текста. NVM Command Set 1.3 и PCIe Transport 1.4 не открывались.

**Сверено с Linux `include/linux/nvme.h` (master, 2026-11-07), не с текстом NVMe Base 2.4:**

- регистры CAP, VS, INTMS, INTMC, CC, CSTS, NSSR, AQA, ASQ, ACQ, doorbells (с `0x1000`);
- биты CC: EN 0, CSS 4, MPS 7, AMS 11, SHN 14, IOSQES 16, IOCQES 20;
- биты CSTS: RDY, CFS, SHST;
- поля CAP: MQES[15:0], TO[31:24], CSS, MPSMIN 48;
- admin-opcode 0x00, 0x01, 0x02, 0x04, 0x05, 0x06, 0x08, 0x09, 0x0A, 0x0C; I/O Flush, Write, Read = 0, 1, 2;
- CNS 0–3;
- FID 0x01, 0x02, 0x04, 0x05, 0x07, 0x08, 0x09, 0x0A, 0x0B;
- LID 0x01–0x03;
- статусы generic 0x01, 0x02, 0x04, 0x0B, 0x0C, 0x13 и command specific 0x100, 0x101, 0x102, 0x105, 0x108, 0x109, 0x10C;
- смещения Identify Controller и Identify Namespace;
- смещения Error log, SMART, Firmware Slot log.

**Оставшиеся вопросы сверки:** коды AER info 0 и 1; правила PRP; значения features по умолчанию; поведение при ошибках; обязательность Admin-команд; `VS` 2.0.0 против 2.4.0. Пометки в заголовках `controller.rs`, `admin.rs` и `queues.rs` отражают первоначальную сверку и будут уточнены при изменении кода. Наименее надёжные места перечислены в разделе 11.

## 2. Размещение и handoff block

- **Окно MMIO:** `[0xFC00_1000, 0xFC00_3000)`, 8 KiB (`NVME_MMIO_BASE`, `NVME_MMIO_BYTES`). Регистры — `0x0000..0x1000`, doorbells — с `0x1000`, шаг 4 байта (`CAP.DSTRD = 0`). Выбранный диапазон лежит в зарезервированном «свободном» участке `[0xFC00_1000, 0xFEC0_0000)` ([FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) 2), сразу за регистрами дисплея, и не меняет число страниц `BOOT_PAGE_TABLES` (тот же блок 2 MiB). После S4-30 окно заменяет BAR0; адрес окна — решение v1, не требование спецификации NVMe.
- **Handoff block.** В v1.0 поля NVMe нет (коды 8–15 зарезервированы). Поэтому устройство добавляется как запись кода 8 и повышает `version_minor` до 1 ([FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) 6.3–6.4): `PRESENT = 1`, MMIO, `base = 0xFC00_1000`, `length = 0x2000`, `gsi = 0xFFFF_FFFF` (нет IRQ), `irq_flags = 0`, `param0 = param1 = 0`; в таблицу регионов добавляется регион `MMIO` с `owner_device = 7`.
- **Включается опционально:** `FirmwareBoardConfig::nvme: Option<NvmeConfig>`. По умолчанию `None`: handoff остаётся v1.0 байт в байт (эталон `REFERENCE_HANDOFF_HEX` не менялся), окна нет, `Unmapped`. При `Some` образ обязан объявлять `handoff_minor_max ≥ 1`, иначе загрузка отклоняется (`ImageHeader { field: "handoff_minor_max" }`). Прошивка в репозитории (`firmware/uefi`) пока понимает только minor 0 и NVMe-запись не читает — это работа S7-17.
- Устройство подключается к шине только когда запись PRESENT; без конфигурации оно не существует и не объявляется.
- **Linux direct boot с `nvme_pci`.** Endpoint `00:04.0` (class `010802`) доступен через ECAM и conf1. 64-битный non-prefetchable BAR0/1 занимает `[0xFC00_4000, 0xFC00_8000)`. MMIO разрешено при MSE и запрограммированном адресе этой апертуры, DMA при BME. Адрес вне апертуры выключает декодирование модели. ACPI публикует окно в `PCI0._CRS` и INTA → GSI 16 в `PCI0._PRT`. Этот режим требует Linux execution profile `X8664V1Stage4AApic`. Привязка драйвера в запущенном госте ещё не проверялась.

### 3.1. Прерывания PCI

С выключенным MSI-X очередь с `IEN = 1` удерживает level/active-low INTA, пока остаются неподтверждённые CQE. На линию влияют PCI Command.INTx Disable и `INTMS/INTMC`. При MSI-X Enable линия INTA снимается на следующей выборке источника. `INTMS/INTMC` управляют только INTA.

MSI-X capability по смещению `0x50` объявляет пять векторов. Таблица из пяти записей по 16 байт расположена с `BAR0 + 0x2000`. PBA расположен с `BAR0 + 0x3000`. Отдельные страницы таблицы и PBA входят в окно PCI. Биты Enable и Function Mask доступны для записи. В каждой табличной записи доступны адрес сообщения, данные и бит Vector Mask. У PBA доступны только чтения. При создании платы все векторы замаскированы, `Enable = Function Mask = 0`, PBA пуст. [PCIe Transport 1.4, §3.8.4](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVMe-over-PCIe-Transport-Specification-Revision-1.4-Ratified-2026.07.31.pdf#page=24).

Admin CQ использует вектор 0, I/O CQ выбирает вектор через `IV` в Create I/O CQ. После успешной записи CQE контроллер формирует событие для соответствующего вектора. Function Mask или Vector Mask удерживают его в PBA. Запись CQ head не очищает PBA. Событие отправляется при снятии обеих масок и наличии BME. `CC.EN` сбрасывает очереди, а состояние MSI-X относится к PCI-функции и сохраняется. Поддерживается Fixed доставка edge-triggered в физический xAPIC ID существующего vCPU. Для сообщения вне этого формата, несуществующей цели или отказа LAPIC флаг PBA остаётся установленным. Повторная доставка пробуется при записи конфигурации, записи таблицы и выборке источника PCI IRQ. [PCIe Transport 1.4, §3.5.2](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVMe-over-PCIe-Transport-Specification-Revision-1.4-Ratified-2026.07.31.pdf#page=15), [Intel SDM Vol. 3A, Message Signalled Interrupts](https://cdrdv2-public.intel.com/812386/253668-sdm-vol-3a.pdf).

## 3. Модель обслуживания (DMA)

`BusDevice::read/write` не имеют доступа к RAM гостя. Поэтому запись в doorbell только запоминает новое значение хвоста/головы очереди и выставляет флаг (`NvmeHandle::doorbell_pending()`). Команды извлекаются из SQ, исполняются, а completion записываются в CQ вызовом `NvmeHandle::service(&mut MachineBus, committed_write)` **между квантами гостя**. `LoadedFirmwareBoard::run_boot` делает это: останавливает квант после doorbell-записи, вызывает `service` после каждого кванта и один раз перед началом. Следствия:

- завершение команды видно гостю не сразу после записи в doorbell, а после ближайшего `service`; драйвер, который опрашивает фазовый бит в CQ (прошивка), это допускает; время виртуальной машины между doorbell и completion не моделируется;
- все команды исполняются полностью внутри одного `service`; поэтому `Abort` никогда не находит команду в работе (раздел 7), а AER — единственный вид команды, который остаётся невыполненным;
- DMA разрешён только в RAM (`MemoryRegionKind::Ram`); PRP, указывающий в MMIO (в том числе в окно самого NVMe), ROM, framebuffer или вне карты, — `Data Transfer Error` команды, а не доступ и не паника. Записи в RAM сообщают диапазон в `committed_write`, как любые другие записи гостевой RAM.
- **Разрыв в тестах (не код).** Путь «гость пишет doorbell → квант останавливается по `doorbell_pending` → `service` между квантами» тестом не покрыт. Причина: интерпретатор поддерживает 64-битный `mov [rbx], rcx`, а doorbell — 32-битный регистр (`Dword`), поэтому гость-тест не может записать doorbell. Doorbell гостем end-to-end не тестируется. Покрыто: `service` на стороне хоста (`run_boot_executes_commands_submitted_before_the_guest_starts` — команды, поданные до старта гостя) и запись в doorbell через шину в тестах модуля `nvme`.
- Очередь SQ/CQ в не-RAM (ASQ/ACQ, I/O-очереди) обнаруживается при обращении: ошибка чтения SQE или записи CQE переводит контроллер в `CSTS.CFS = 1`; выход — сброс (`CC.EN = 0`).

## 4. Регистры контроллера

Доступ — только `Dword` на 4-байтовых регистрах; 64-битные `CAP`, `ASQ`, `ACQ` — `Qword` по выровненному адресу либо двумя `Dword` (младшая и старшая половины). Другая ширина — `UnsupportedWidth`; невыровненный адрес — `Misaligned`; `Qword` через границу регистров — `CrossesRegister`; запись в регистр только для чтения — `ReadOnly`; запись в `AQA/ASQ/ACQ` при `CC.EN = 1` — `Busy` (значение не меняется). Чтение зарезервированных регистров (`0x18`, `0x20` `NSSR`, `0x38..0x1000`: CMB, boot partition, PMR) даёт 0, запись игнорируется. Значения сверены с Linux `include/linux/nvme.h`, кроме отмеченных NOT VERIFIED в разделе 1.

| Смещение | Регистр | Реализация |
|---|---|---|
| `0x00` | `CAP` (RO, 64) | `MQES = 1023` (1024 записи); `CQR = 1` (только физически непрерывные очереди); `AMS = 0` (только round robin); `TO = 1` (500 мс; `RDY` меняется синхронно); `DSTRD = 0`; `NSSRS = 0`; `CSS`: бит 37 (NVM command set); `MPSMIN = MPSMAX = 0` (страница 4 KiB); `BPS`, `CPS`, `PMRS`, `CMBS`, `NSSS`, `CRMS` = 0. Значение `0x0000_0020_0101_03FF` |
| `0x08` | `VS` (RO) | `0x0002_0000` (2.0.0). Заявлена 2.0.0, а не 2.4.0: набор реализованного (раздел 11) не претендует на более новую редакцию |
| `0x0C` | `INTMS` (RW1S) | Маска pin-based прерываний. Запись 1 ставит бит, чтение возвращает маску. Бит 0 маскирует INTA в PCI-профиле при выключенном MSI-X |
| `0x10` | `INTMC` (RW1C) | Запись 1 снимает бит; чтение возвращает ту же маску |
| `0x14` | `CC` (RW) | Записываемые биты: `EN`, `CSS[6:4]`, `MPS[10:7]`, `AMS[13:11]`, `SHN[15:14]`, `IOSQES[19:16]`, `IOCQES[23:20]`. `CRIME` не реализован. Для включения нужны `CSS = 0`, `MPS = 0` и `AMS = 0`. При создании I/O-очереди поддерживаются `IOSQES = 6` (SQE 64 байта) и `IOCQES = 4` (CQE 16 байт) |
| `0x1C` | `CSTS` (RO) | `RDY` бит 0, `CFS` бит 1, `SHST[3:2]`; `NSSRO`, `PP` = 0 |
| `0x20` | `NSSR` | не реализован (`CAP.NSSRS = 0`), чтение 0, запись игнорируется |
| `0x24` | `AQA` | `ASQS[11:0]` и `ACQS[27:16]`, 0-based; допустимо 2…1024 записей |
| `0x28` | `ASQ` (64) | база admin SQ; биты `[11:0]` читаются как 0 |
| `0x30` | `ACQ` (64) | база admin CQ; биты `[11:0]` читаются как 0 |
| `0x1000+` | doorbells | раздел 5 |

**Включение и сброс.**

- При переходе `CC.EN` 0 → 1 проверяются `CSS/MPS/AMS` и размеры `AQA`. При успехе создаются пустые admin SQ/CQ с записями 64 и 16 байт соответственно, `CSTS = RDY`. При ошибке `CSTS = CFS`, `RDY = 0`, очереди не создаются (поведение при неверных значениях — NOT VERIFIED, раздел 11). Выход из `CFS` — запись `CC.EN = 0`.
- `CC.EN` 1 → 0 — сброс контроллера. Очереди, features, невыполненные AER и записи Error Information очищаются, `CSTS = 0`. Идентификатор последней ошибки и счётчик SMART сохраняются в экземпляре контроллера. [NVMe Base 2.4, §5.2.13.1.2](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=244)
- При `CC.EN = 1` можно записать `SHN` и независимо менять `IOSQES` и `IOCQES`, пока отсутствуют I/O-очереди соответствующего типа. При наличии таких очередей результат изменения поля спецификацией не определён. Модель сохраняет прежнее значение поля до удаления последней очереди этого типа. Запись `SHN = 01b/10b` сразу выставляет `CSTS.SHST = 10b` (shutdown complete).
- Контроллер не «всегда готов»: до `CC.EN = 1` `RDY = 0`, doorbells возвращают `UnsupportedConfiguration`, `service` ничего не делает.

## 5. Doorbells

Doorbell очереди `y`: SQ tail — `0x1000 + 8·y`, CQ head — `0x1000 + 8·y + 4`; `y = 0` — admin; `y = 1..4` — I/O (`MAX_IO_QUEUES = 4`). Только `Dword` (`UnsupportedWidth`), выровненный адрес (`Misaligned`). Чтение — `WriteOnly`. Смещение за doorbell очереди 4 (`≥ 0x1028`) — `UnknownOffset` (чтение и запись). Запись при `RDY = 0` — `UnsupportedConfiguration`. Значение — младшие 16 бит.

- Doorbell несозданной очереди (в пределах `y ≤ 4`): запись принимается, значение не используется; в Error Information log добавляется запись, контроллер отправляет асинхронное событие «Write to Invalid Doorbell Register» (тип 0, info 0, log page 1).
- Значение `≥` размера очереди: запись принимается, значение игнорируется, запись в Error Information log, событие «Invalid Doorbell Write Value» (тип 0, info 1, log page 1).
- Остальные значения обновляют `tail` (SQ) или `head` (CQ) и помечают работу как ожидающую.

## 6. Очереди, команды и completion

- SQE — 64 байта, CQE — 16 байт. Очереди физически непрерывные (`PC = 1`), база выровнена по странице 4 KiB; размер 2…1024 записей. Admin — очередь 0; I/O: до четырёх пар. Идентификаторы SQ и CQ — разные пространства; число очередей по умолчанию 4 (`Set Features / Number of Queues` может уменьшить, пока нет I/O-очередей).
- Команда: `OPC[7:0]`, `FUSE[9:8]`, `PSDT[15:14]`, `CID[31:16]`; `NSID`, `PRP1/PRP2`, `CDW10..13` (`MPTR` и `CDW2/3/14/15` не используются). `FUSE ≠ 0` и `PSDT ≠ 0` (SGL) — `Invalid Field in Command`.
- CQE: `DW0` — команда-специфично; `DW2`: `SQHD[15:0]` (голова SQ после извлечения команды), `SQID[31:16]`; `DW3`: `CID`, фазовый бит `P` (бит 16), `SC[24:17]`, `SCT[27:25]`, `DNR` (бит 31); `CRD`, `M` = 0.
- Фазовый бит: CQ создаётся с `P = 1` и с нулевыми записями гостя; контроллер пишет запись целиком одной операцией и переворачивает `P` при каждом проходе `tail` через конец очереди. CQ считается заполненной, когда остаётся один свободный слот (`tail + 1 == head`): `service` перестаёт извлекать команды из SQ этой CQ, пока гость не сдвинет `head` doorbell'ом; запись не затирается.
- Round robin обслуживает одну команду из каждой готовой SQ за круг, включая Admin SQ. Посещения идут по номеру SQ, порядок команд внутри одной SQ сохраняется. Эффективный burst равен 1 и укладывается в ограничения `Arbitration Burst`. [NVMe Base 2.4, §3.4.4](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=129)
- Ошибки команды — статус NVMe в CQE; `DNR = 1` для всех, кроме `Data Transfer Error` (повтор разрешён) — соглашение v1, NOT VERIFIED.
- **PRP.** Поддержаны `PRP1` со смещением (кратно 4 байтам), `PRP2` как вторая страница или как PRP list (в том числе со смещением внутри страницы), цепочка листов через последний элемент страницы; элементы листа и `PRP2`-страница обязаны быть выровнены по 4 KiB (`PRP Offset Invalid`, SC `0x13`); не более 1024 страниц на команду. Максимум передачи для I/O-команд: `MDTS = 8` (256 страниц, 1 MiB; одна страница PRP list вмещает 512 элементов, цепочка листов для этого размера не нужна). SGL не поддержан (`SGLS = 0`).

## 7. Admin-команды (срез A)

Для `Create/Delete I/O SQ/CQ`, `Abort`, `Asynchronous Event Request`, `Identify Controller` и реализованных controller-scoped Features требуется `NSID = 0`, иначе `Invalid Field`. `Error Recovery` остаётся namespace-scoped и применяет отдельную проверку (раздел 9.1); Identify Namespace/List также сохраняют собственные правила раздела 8.

Delete I/O SQ сначала обслуживает команды, уже покрытые текущим SQ tail, и публикует их CQE до успешного CQE удаления. Если связанная CQ не имеет места для completion всех поданных команд, оставшиеся SQ entries implicitly aborted без исполнения и I/O CQE; SQ удаляется успешно. Неподтверждённый CQ-full статус не вводится.

Мандаторные Admin-команды NVMe Base для контроллера ввода-вывода (по знанию авторов; обязательность NOT VERIFIED, сами opcode сверены с Linux `nvme.h`): `Delete I/O SQ`, `Create I/O SQ`, `Get Log Page`, `Delete I/O CQ`, `Create I/O CQ`, `Identify`, `Abort`, `Set Features`, `Get Features`, `Asynchronous Event Request`. Все десять реализованы. Любой другой opcode — `Invalid Command Opcode` (SC `0x01`, DNR), в том числе опциональные команды, которые контроллер не объявляет в `OACS`/`ONCS` (раздел 10).

| Opcode | Команда | Поведение и границы |
|---|---|---|
| `0x00` | Delete I/O SQ | `CDW10[15:0]` = QID. QID 0, больше 4 или не создана — `Invalid Queue Identifier` (SCT 1, SC `0x01`) |
| `0x01` | Create I/O SQ | QID — `1..=NSQA+1`, не занят. `CDW10[31:16]` = QSIZE (0-based), `CDW11`: `PC` (бит 0, обязан быть 1), `CQID[31:16]`. Порядок проверок: QID → CQID (нет такой CQ — `Completion Queue Invalid`, SCT 1, SC `0x00`) → `IOSQES = 6` и размер (2…1024, иначе `Invalid Queue Size`, SC `0x02`) → `PC = 0` или `PRP1` не выровнен по странице (`Invalid Field`). `QPRIO` игнорируется (WRR нет). Область памяти очереди при создании не проверяется |
| `0x02` | Get Log Page | раздел 9 |
| `0x04` | Delete I/O CQ | как Delete SQ; если к CQ привязана SQ — `Invalid Queue Deletion` (SCT 1, SC `0x0C`) |
| `0x05` | Create I/O CQ | QID — `1..=NCQA+1`, не занят. `CDW11` содержит `PC` (обязан быть 1), `IEN` (бит 1), `IV[31:16]`. Порядок проверок — QID → `IOCQES = 4` и размер (2…1024, иначе `Invalid Queue Size`, SC `0x02`) → `PC`/выравнивание → при `IEN = 1` допустимый `IV` (`0` в режиме INTx, `0..=4` при MSI-X, иначе `Invalid Interrupt Vector`, SCT 1, SC `0x08`). Профиль firmware MMIO использует опрос |
| `0x06` | Identify | раздел 8 |
| `0x08` | Abort | `CDW10`: `SQID[15:0]`, `CID[31:16]`. Несуществующая SQ — `Invalid Field`. Иначе успех с `DW0` бит 0 = 1 (команда не прервана): к моменту исполнения `Abort` все ранее извлечённые команды уже завершены (раздел 3). Лимит одновременных `Abort` объявлен `ACL = 3` и не может быть превышен. Прерывать нечего: команд в работе нет |
| `0x09` | Set Features | раздел 9 |
| `0x0A` | Get Features | раздел 9 |
| `0x0C` | Asynchronous Event Request | принимается и остаётся в очереди без completion, пока не произойдёт событие (раздел 9). Одновременно не более 4 (`AERL = 3`); пятый — `Asynchronous Event Request Limit Exceeded` (SCT 1, SC `0x05`). Сброс контроллера отбрасывает невыполненные AER |

Коды статусов (сверены с Linux `include/linux/nvme.h`, не с NVMe Base 2.4): `Invalid Command Opcode 0/0x01`, `Invalid Field in Command 0/0x02`, `Data Transfer Error 0/0x04`, `Invalid Namespace or Format 0/0x0B`, `Command Sequence Error 0/0x0C`, `PRP Offset Invalid 0/0x13`; command specific (SCT 1): `Completion Queue Invalid 0x00`, `Invalid Queue Identifier 0x01`, `Invalid Queue Size 0x02`, `Asynchronous Event Request Limit Exceeded 0x05`, `Invalid Interrupt Vector 0x08`, `Invalid Log Page 0x09`, `Invalid Queue Deletion 0x0C`.

## 8. Identify

Обе формы Identify Controller (`0x01`, `0x06`) требуют `NSID = 0` (`Invalid Field` при ненулевом NSID). Формы `0x05`–`0x07` проверяют `CDW11[31:24]` (CSI). Поддержан NVM (`CSI = 0`), другой CSI даёт `Invalid Field in Command`. [NVMe Base 2.4, §5.2.14](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=362)

`CDW10[7:0]` = CNS; результат — 4096 байт по PRP, остальное содержимое страницы — нули. Поддержаны:

| CNS | Структура | Границы |
|---|---|---|
| `0x00` | Identify Namespace | `NSID = 1`. Другое (0, 2…, `0xFFFF_FFFE`, `0xFFFF_FFFF`) — `Invalid Namespace or Format` |
| `0x01` | Identify Controller | раздел 8.1 |
| `0x02` | Список активных NSID | `[1]` при `NSID < 1`, иначе пустой (нули); `NSID ≥ 0xFFFF_FFFE` — `Invalid Namespace or Format` |
| `0x03` | Список Namespace Identification Descriptor | `NSID = 1`: дескриптор `NIDT = 2` (NGUID, 16 байт), затем `NIDT = 4` (CSI = 0, NVM command set), затем нулевой терминатор |
| `0x05` | NVM I/O Command Set Specific Identify Namespace | `NSID = 1`. Все поля нулевые для LBA без metadata и protection information. Остальные NSID дают `Invalid Namespace or Format` |
| `0x06` | NVM I/O Command Set Specific Identify Controller | Все поля нулевые. Дополнительные лимиты и варианты NVM-команд не объявлены |
| `0x07` | Список активных NSID для NVM | Правила `0x02`, поскольку единственное пространство имён принадлежит NVM |
| `0x08` | I/O Command Set Independent Identify Namespace | `NSID = 1`. Бит `RESCAP.IEKS` установлен согласно версии, типы резерваций не объявлены. `NSTAT.NRDY = 1` при подключённом backend. Остальные NSID дают `Invalid Namespace or Format` |

Остальные CNS (в том числе `0x1C`, NVM Set, secondary controller, allocated namespace list) дают `Invalid Field in Command`. [NVM Command Set 1.3, §4.1.5](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-Revision-1.3-Ratified-2026.07.31.pdf#page=83)

### 8.1. Identify Controller — объявленные значения

Остальные поля — 0 (не поддержано либо значение «не сообщается»).

| Смещение | Поле | Значение | Смысл |
|---|---|---|---|
| 0, 2 | `VID`, `SSVID` | 0 | **PCI vendor ID проекту не выдан**; поле определяется вместе с PCI-привязкой (S4-30). Это зафиксированная заглушка, не значение «по смыслу» |
| 4 | `SN` (20) | `NvmeConfig::serial`, по умолчанию `OD-NVME-0001` + пробелы | ASCII `0x20..=0x7E`, дополнено пробелами |
| 24 | `MN` (40) | `Open Desktop Virtual NVMe` + пробелы | |
| 64 | `FR` (8) | `0.1.0` + пробелы | |
| 72 | `RAB` | 0 | burst арбитража 1 |
| 73 | `IEEE` (3) | 0 | OUI проекту не выдан |
| 76 | `CMIC` | 0 | один контроллер, без multi-path/SR-IOV |
| 77 | `MDTS` | 8 | максимум 2^8 страниц = 1 MiB |
| 78 | `CNTLID` | 0 | |
| 80 | `VER` | `0x0002_0000` | совпадает с `VS` |
| 92 | `OAES` | 0 | необязательных асинхронных событий нет |
| 96 | `CTRATT` | 0 | |
| 111 | `CNTRLTYPE` | 1 | I/O controller |
| 256 | `OACS` | 0 | нет Security, Format NVM, Firmware, Namespace Management, Device Self-test и других необязательных Admin-команд |
| 258 | `ACL` | 3 | четыре `Abort` (0-based) |
| 259 | `AERL` | 3 | четыре AER (0-based) |
| 260 | `FRMW` | `0x03` | слот 1 только для чтения; один слот |
| 261 | `LPA` | `0x06` | Поддерживаются расширенный `NUMD`, байтовый `LPO` и Commands Supported and Effects. `MLPS = 0` допускает страницы `0x00` и `0x12` без объявления областей действия команд в `0x05`. SMART доступен для всего контроллера |
| 262 | `ELPE` | 63 | 64 записи Error Information log |
| 263 | `NPSS` | 0 | одно состояние питания |
| 266, 268 | `WCTEMP`, `CCTEMP` | 343, 358 K | пороги предупреждения и критический |
| 280 | `TNVMCAP` (16) | `namespace_sectors · 512` | `UNVMCAP = 0` (нет Namespace Management) |
| 512 | `SQES` | `0x66` | только SQE 64 байта |
| 513 | `CQES` | `0x44` | только CQE 16 байт |
| 516 | `NN` | 1 | |
| 520 | `ONCS` | 0 | необязательных NVM-команд нет: `Read`/`Write`/`Flush` в `ONCS` не объявляются (NOT VERIFIED: какие команды обязательны); Compare, Write Uncorrectable, Dataset Management, Write Zeroes, Verify, Copy — не реализованы |
| 522 | `FUSES` | 0 | fused-операций нет |
| 525 | `VWC` | `0x04` | `VWCP = 0`: энергозависимого кэша записи нет. `FB = 10b`: `Flush` с `NSID = 0xFFFF_FFFF` даёт `Invalid Namespace or Format`. С `NSID = 1` вызывается `flush` backend |
| 536 | `SGLS` | 0 | SGL не поддержан |
| 768 | `SUBNQN` | `nqn.2014-08.org.nvmexpress:uuid:<UUID>` | UUID из `NvmeConfig::subsystem_uuid`, дополнено нулями |
| 2048 | `PSD0` | `MP = 2500` (25,00 Вт), задержки 0 | описание единственного состояния питания |

### 8.2. Identify Namespace (NSID 1)

`NSZE = NCAP = NUSE = namespace_sectors` (поддержки thin provisioning нет); `NSFEAT = 0`; `NLBAF = 0` (один формат LBA); `FLBAS = 0`; `NVMCAP = namespace_sectors · 512`; `NGUID = namespace_uuid`; `EUI64 = 0`; `LBAF0`: `MS = 0`, `LBADS = 9` (512 байт), `RP = 0`. **Решение о 4 KiB LBA:** в v1 формат один, 512 байт; формат 4096 не объявляется (сектор 512 байт — единица `od-storage` и размер, который принимают `stornvme`, прошивка и GPT S7-17). Расширение — отдельное решение до среза B.

## 9. Features и log pages

### 9.1. Features

Controller-scoped FID требуют `NSID = 0`; `Error Recovery (0x05)` проверяет namespace (0, 1 или `0xFFFF_FFFF`).

`CDW10[7:0]` = FID; Get — `CDW10[10:8]` = SEL: поддержаны `0` (текущее) и `1` (значение по умолчанию); `2` (сохранённое) и `3` (возможности) — `Invalid Field`. Set — бит `SV` (31): сохранение не поддержано (`ONCS` бит 4 = 0), `SV = 1` — `Invalid Field`. Features сбрасываются к значениям по умолчанию сбросом контроллера. Неподдержанный FID — `Invalid Field`.

| FID | Feature | Поддержка |
|---|---|---|
| `0x01` | Arbitration | сохраняет `AB`, `LPW/MPW/HPW`; по умолчанию `AB = 7`. Проход round robin выбирает по одной команде на SQ. Приоритеты WRR вне выбранного механизма |
| `0x02` | Power Management | только `PS = 0`, без workload hint; иное — `Invalid Field` |
| `0x04` | Temperature Threshold | Единственный датчик composite (`TMPSEL = 0`). Set с `TMPSEL = 0xF` также применяет порог composite. Over (`THSEL = 0`) по умолчанию 343 K, under (`THSEL = 1`) по умолчанию 0 K. Неподдерживаемый селектор или ненулевой `TMPTHH` даёт `Invalid Field`. Достижение или пересечение порога устанавливает `SMART.TTC` |
| `0x05` | Error Recovery | `TLER[15:0]`, `DULBE` не поддержан (`Invalid Field`); `NSID` — 0, 1 или `0xFFFF_FFFF`, иначе `Invalid Namespace or Format` |
| `0x07` | Number of Queues | Get — текущее число (по умолчанию 4 SQ и 4 CQ, 0-based `3`, `DW0 = NSQA \| NCQA << 16`); Set — `NSQR/NCQR`, значение больше доступного усекается, `0xFFFF` — `Invalid Field`; после создания любой I/O-очереди — `Command Sequence Error` |
| `0x08` | Interrupt Coalescing | `THR/TIME` хранятся, на доставку не влияют |
| `0x09` | Interrupt Vector Configuration | `IV = 0` в режиме INTx, `IV = 0..=4` при MSI-X. Set Features в режиме MSI-X требует I/O CQ с `IEN = 1` и соответствующим вектором. `CD` хранится отдельно для каждого вектора. Get Features возвращает `IV` и `CD` |
| `0x0A` | Write Atomicity Normal | `DN` хранится |
| `0x0B` | Asynchronous Event Configuration | Допустимы биты `[7:0]` SMART/Health critical warning. Бит 1 разрешает событие Temperature Threshold. `OAES = 0` |

Не поддержаны, `Invalid Field`: `0x03` LBA Range Type, `0x06` Volatile Write Cache (`VWC.VWCP = 0`), `0x0C` APST, `0x0D…0x0F`, `0x80` Software Progress Marker и все остальные.

### 9.2. Get Log Page

Команда читает `(NUMD + 1) · 4` байт с байтового смещения `LPO`. `CDW10` содержит `LID[7:0]`, `RAE` (бит 15) и `NUMDL[31:16]`. `CDW11[15:0]` содержит `NUMDU`. `CDW12/CDW13` задают смещение, кратное 4. Для поддерживаемых страниц допустимы `NSID = 0` и `0xFFFF_FFFF`. `CDW14.OT = 1`, смещение больше размера страницы или длина больше `MDTS` дают `Invalid Field`. Для `0x00`, `0x05` и `0x12` ненулевой `UIDX` также даёт `Invalid Field`. Неизвестный `LID` даёт `Invalid Log Page` (SCT 1, SC `0x09`). Контроллер заполняет нулями участок буфера за концом страницы. [ISSUE-066](../KNOWN_ISSUES.md#issue-066-s7-16-nvme-spec-values-not-verified).

| LID | Страница | Содержимое |
|---|---|---|
| `0x00` | Supported Log Pages (1024) | 256 little-endian `u32`, по одному на LID. `LSUPP = 1` для шести страниц в таблице. Индексы записей как `LPO` и параметры `LIDSP` не объявлены |
| `0x01` | Error Information | До 64 записей по 64 байта, новейшая первой. Ошибочный CQE со `M = 1` и запись содержат общий статус. Ошибка параметра указывает байт и бит начала поля SQE в `Parameter Error Location`. Если конкретное поле команды неприменимо, значение нулевое. Ошибка doorbell вне команды даёт `SQID = CID = Parameter Error Location = 0xFFFF`. `LBA Out of Range` указывает первый недопустимый LBA. `NSID` берётся из команды (`0xFFFF_FFFF`, если 0). `Error Count` начинается с 1 и после `u64::MAX` начинается снова. Неиспользованные записи нулевые. [NVMe Base 2.4, Figure 212](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=245) · [NVM Command Set 1.3, Figure 110](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-Revision-1.3-Ratified-2026.07.31.pdf#page=76) |
| `0x02` | SMART / Health Information (512) | `Critical Warning.TTC` определяется порогами FID `0x04`. `Composite Temperature = 300` K; `Available Spare = 100`, `Threshold = 10`; `Percentage Used = 0`; `Data Units Read/Written` (смещения 32/48) — тысячи секторов по 512 байт, с округлением вверх, `Host Read/Write Commands` (64/80) — число успешных `Read`/`Write` с включения питания (срез B; NOT VERIFIED: единицы и округление); `Controller Busy Time` = 0 (нет времени); `Power Cycles = 1`; `Number of Error Information Log Entries` — живой счётчик ошибок; остальное 0. **`Power On Hours`, `Controller Busy Time` = 0, так как у устройства нет источника времени** — разрыв |
| `0x03` | Firmware Slot Information (512) | `AFI = 0x01` (активен слот 1, следующий — нет); `FRS1` = `FR`; слоты 2–7 пусты (нули) |
| `0x05` | Commands Supported and Effects (4096) | 256 little-endian `u32` для Admin с байта 0 и 256 для NVM I/O с байта 1024. `CSUPP = 1` для реализованных команд, `LBCC = 1` для NVM `Write`. Зарезервированный остаток обнулён |
| `0x12` | Feature Identifiers Supported and Effects (1024) | 256 little-endian `u32`, по одному на FID. `FSUPP = 1` для реализованных features. `FSP` указывает namespace для Error Recovery и controller для остальных. Зарезервированные поля обнулены |

Успешное чтение с `RAE = 0` очищает Error event для `0x01` и SMART/Health event для `0x02`. Остальные поддержанные журналы не сбрасывают эти события. Поскольку `CC.CSS = 0`, `CSI` не влияет на эти три страницы. [Base 2.4, Figures 204–211](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=239), [Figures 216–217](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=253), [Figures 270–271](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=302), [NVM Command Set 1.3, Figure 92](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-Revision-1.3-Ratified-2026.07.31.pdf#page=64).

### 9.3. Асинхронные события

- Ошибочные doorbell создают Error event (тип 0, `info` 0 или 1, `LID = 0x01`).
- Переход к разрешённому предупреждению `TTC` создаёт SMART/Health event (тип 1, `info = 1`, `LID = 0x02`). Постоянная composite temperature равна 300 K. Порог можно пересечь изменением настройки; включение уведомления при действующем предупреждении тоже создаёт событие.
- `DW0` completion AER содержит тип, `info << 8` и `LID << 16`. Если AER ещё нет, событие ждёт запроса. Успешное чтение связанного журнала с `RAE = 0` очищает уже доставленное или ожидающее событие. Следующее пересечение порога может создать новое SMART-событие. [NVMe Base 2.4, §5.2.2](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-Revision-2.4-Ratified-2026.07.31.pdf#page=209)

## 10. Что не сделано и почему

| Что | Причина и статус |
|---|---|
| I/O `Read`, `Write`, `Flush` | реализованы в срезе B (раздел 13). Остальные NVM-команды (`Write Zeroes`, `Dataset Management`, `Compare`, `Write Uncorrectable`, `Verify`, `Copy`) — `Invalid Command Opcode`; обязательность и состав набора для заявляемой редакции не сверены (раздел 11) |
| Файловый backend / CoW-образ | `od-machine` содержит только trait `NvmeBackend` и `InMemoryBackend` (раздел 13); подключение `od-storage` — за владельцем машины |
| Namespace Management/Attachment, несколько NS | вне scope S7-16; один NS `NSID = 1`, `OACS` бит 3 = 0 |
| Firmware Image Download/Commit | вне scope; `FRMW` объявляет слот 1 только для чтения |
| Device Self-test, Format NVM, Sanitize, Security Send/Receive, Keep Alive, Directives, Virtualization, Doorbell Buffer Config | вне scope; `Invalid Command Opcode` |
| SGL | вне scope; `PSDT ≠ 0` — `Invalid Field`, `SGLS = 0` |
| Прерывания в профиле firmware MMIO | `gsi = 0xFFFF_FFFF`. Прошивка опрашивает CQ |
| Доставка MSI-X в PCI-профиле | Fixed edge в физический xAPIC ID. Гостевые драйверы и другие форматы сообщений остаются непроверенными |
| Changed Attached Namespace List, телеметрия | `Invalid Log Page` |
| Время (`Power On Hours`, `Controller Busy Time`, `TO`) | у устройства нет виртуальных часов; `RDY` выставляется синхронно, объявлено `TO = 1` |
| `NSSR` (NVM subsystem reset), `CRIME`, CMB, PMR, boot partitions, WRR | вне реализованного профиля; `CAP` их не объявляет |
| Идентификаторы `VID`, `SSVID`, `IEEE` | нулевые; значения проекту не выданы (раздел 8.1) |
| Моделирование времени исполнения команд | команды завершаются в `service`, без виртуальной задержки |

## 11. NOT VERIFIED: места, где расхождение наиболее вероятно

Сверка с Linux `nvme.h` (раздел 1) не заменяет сверку с текстом NVMe Base 2.4; NOT VERIFIED остаётся всё, что в разделе 1 не перечислено как сверенное. Особо проверить при сверке:

- положение и значения полей `CAP` (`MQES`, `TO`, `CSS` бит 37, `MPSMIN/MPSMAX`) и значение `VS` для заявляемой редакции (2.0.0 против 2.4.0; согласовать с `Identify Controller.VER`);
- битовые поля `CC`, `CSTS`, `AQA` и поведение при недопустимых значениях `CC` (здесь — `CSTS.CFS = 1`, `RDY = 0`) и при записи `AQA/ASQ/ACQ` при `CC.EN = 1` (здесь — `Busy` на шине, без эффекта);
- смещения и размеры остальных полей Identify Controller / Namespace, `FRMW`, `LPA`, `SQES/CQES`, `SUBNQN`, `PSD0`, значение `CNTRLTYPE`. Применимые `CNS 0x00`–`0x03` и `0x05`–`0x08` сверены в Figure 336;
- перечень обязательных Admin-команд и обязательных features для контроллера ввода-вывода PCIe (раздел 7, 9.1), значения по умолчанию features (`Arbitration = 7`, порог 343 K), допустимость `SEL ≠ 0`;
- коды статусов и их SCT, особенно `PRP Offset Invalid`, `Invalid Interrupt Vector`, `Invalid Queue Deletion`, порядок проверок при создании очередей, `DNR`;
- применимость прочих расширенных полей Error Information к отдельным ошибкам и сохранение счётчиков через новый экземпляр контроллера. Проверенные поля `Parameter Error Location` и `LBA` описаны в разделе 9.2;
- расширенные правила AER для других типов событий вне текущего профиля. Маскирование и `RAE` для Error и SMART описаны в разделе 9.3;
- ответ `Abort`, когда цель не найдена (`DW0` бит 0 = 1), и поведение при несуществующей SQ;
- реакция на doorbell несозданной очереди и на значение вне диапазона (здесь — запись принимается, событие и запись в журнал, а не ошибка шины);
- правила PRP: допустимое смещение `PRP1` и `PRP2`-листа, цепочка листов.

**Открытые вопросы среза B (для ревью и S7-17):** см. раздел 13.5.

**Открытые вопросы среза A (для ревью и S7-17):**

1. `VS = 2.0.0` (и `Identify Controller.VER`) против 2.4.0.
2. Запись doorbell гостем не покрыта тестом end-to-end (раздел 3).
3. Коды AER info, правила PRP, умолчания features, поведение при ошибках — NOT VERIFIED (раздел 1).

## 12. Проверка

`cargo test -p od-machine --offline` — модуль `nvme` (регистры, ошибки доступа, `CC.EN/RDY/CFS`, Identify побайтно, active NS list, создание/удаление очередей с фазовым битом и заполненной CQ, ошибка создания SQ без CQ, Features, три log page, AER, Abort, неверный doorbell, неподдержанная команда, PRP на границе страницы и со списком) и `firmware_board` (handoff v1.1 с NVMe, окно не отображено без устройства, `run_boot` выполняет заранее поданные команды; путь «гость пишет doorbell → остановка кванта → `service`» end-to-end не покрыт, раздел 3). Срез B: тесты `nvme/tests.rs` (раздел «I/O») и `nvme/backend.rs`, `nvme/queues.rs` (`read_from_host`): Write → Read через PRP1+PRP2 и PRP list, максимум 1 MiB, Flush, LBA/NSID/NLB/поля/PRP-ошибки, ошибка backend, нет backend, полная CQ, отсутствующая SQ, счётчики SMART. Тесты, зависящие от значений раздела 1, проверяют только то, что объявлено здесь; они не доказывают соответствие спецификации (сверка — с Linux `nvme.h`, раздел 1).

## 13. Срез B: I/O

Код: `nvme/io.rs` (команды), `nvme/backend.rs` (trait и `InMemoryBackend`), `nvme/controller.rs` (`attach_backend`, исполнение команд очереди I/O). I/O-команды берутся из SQ 1…4 тем же циклом `service`, что и admin (раздел 6): порядок, фазовый бит, полная CQ, `SQHD` — без изменений.

### 13.1. Команды

Раскладка `Read` и `Write` сверена с NVM Command Set 1.3. Для условий и DNR отдельных статусов остаются пункты сверки в разделе 13.5. [Формат Read](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-Revision-1.3-Ratified-2026.07.31.pdf#page=49) · [Формат Write](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-Revision-1.3-Ratified-2026.07.31.pdf#page=54)

| Поле | Read / Write |
|---|---|
| `NSID` | только 1 |
| `PRP1`, `PRP2` | данные; правила PRP — раздел 6 |
| `CDW10`, `CDW11` | `SLBA` (младшие, старшие 32 бита) |
| `CDW12[15:0]` | `NLB`, 0-based: 1…2048 секторов (512 байт … 1 MiB, `MDTS = 8`) |
| `CDW12[31:16]` | `FUA` (бит 30) и `LR` (31) допустимы. Backend работает в режиме write-through, контроллер выполняет одно обращение к backend. Для `Write` `DTYPE` игнорируется при выключенных директивах. Остальные биты должны быть нулевыми |
| `CDW13` | `DSM[7:0]` содержит подсказки, которые контроллер не использует для оптимизации. `AF[3:0]` допустим от 0 до 8 для `Read` и от 0 до 6 для `Write`. Для `Write` `DSPEC[31:16]` игнорируется при выключенных директивах. Остальные биты должны быть нулевыми |
| `CDW14`, `CDW15`, `MPTR` | игнорируются: защиты данных (`PRINFO = 0`) и метаданных (`MS = 0`) нет |

`Flush`: `NSID = 1`; `PRP`, `CDW10..15` игнорируются. `NSID = 0xFFFF_FFFF` (все пространства имён) не принимается: `Invalid Namespace or Format`.

**Порядок проверок** (первая ошибка — статус команды): opcode (`Invalid Command Opcode`) → `FUSE ≠ 0` или `PSDT ≠ 0` (`Invalid Field`) → `NSID ≠ 1` (`Invalid Namespace or Format`) → backend не подключён (`Namespace Not Ready`) → (Read/Write) недопустимые биты в `CDW12`/`CDW13` или зарезервированное значение `AF` (`Invalid Field`) → `NLB + 1 > 2048` (`Invalid Field`) → `SLBA + NLB + 1 > NSZE` с проверкой переполнения 64 бит (`LBA Out of Range`) → передача.

**Передача.** `Read`: чтение из backend, затем запись в память гостя по PRP. `Write`: сначала весь буфер читается из памяти гостя по PRP, потом пишется в backend; при ошибке PRP диск не меняется. Память гостя — только RAM (раздел 3).

### 13.2. Статусы

| Статус | SCT/SC | DNR | Когда | Источник значения |
|---|---|---|---|---|
| Success | 0/`0x00` | | | |
| Invalid Command Opcode | 0/`0x01` | 1 | неизвестный I/O-opcode | значение `nvme.h`; DNR и условие NOT VERIFIED |
| Invalid Field in Command | 0/`0x02` | 1 | поля выше, `PSDT`, `FUSE`, `NLB` больше `MDTS`, `PRP1` не кратен 4 | значение `nvme.h`; DNR и условие NOT VERIFIED |
| Data Transfer Error | 0/`0x04` | 0 | PRP, список PRP или данные вне RAM | значение `nvme.h`; DNR и условие NOT VERIFIED |
| Invalid Namespace or Format | 0/`0x0B` | 1 | `NSID ≠ 1` | значение `nvme.h`; DNR и условие NOT VERIFIED |
| PRP Offset Invalid | 0/`0x13` | 1 | `PRP2` или элемент списка не выровнен по 4 KiB | значение `nvme.h`; DNR и условие NOT VERIFIED |
| LBA Out of Range | 0/`0x80` | 1 | выход за `NSZE` | значение `nvme.h`; DNR и условие NOT VERIFIED |
| Internal Error | 0/`0x06` | 0 | `BackendError` любого вида при обращении к диску | значение `nvme.h`; DNR и условие NOT VERIFIED (медиа-ошибки SCT 2 `Write Fault`/`Unrecovered Read Error` не используются) |
| Namespace Not Ready | 0/`0x82` | 0 | backend не подключён | значение `nvme.h`; DNR и условие NOT VERIFIED |

Каждая неуспешная команда попадает в Error Information log (раздел 9.2). Паник нет: любая ошибка PRP, диапазона, поля или backend — статус.

### 13.3. Backend и namespace

- `trait NvmeBackend`: `sector_count`, `read_sectors(lba, &mut [u8])`, `write_sectors(lba, &[u8])`, `flush`; буфер — целое число секторов по 512 байт; ошибки — `BackendError` (`OutOfRange`, `Io`, `InvalidSize`), не паника. Контроллер проверяет диапазон сам, но реализация обязана отвергать выход за пределы.
- `InMemoryBackend` (`new(sectors)`, `from_bytes`, `as_bytes`, `flush_count`) — нулевой вектор фиксированного размера для тестов и мини-образов. **Точка интеграции host-файла:** владелец машины реализует `NvmeBackend` над контрактами `od-storage` (`STORAGE_FORMAT.md`) и передаёт его в `NvmeHandle::attach_backend(Box<dyn NvmeBackend + Send>)`; host-контракт Stage 5 описан в §14. Файлового ввода-вывода в `od-machine` нет. Для текущего `VWC.VWCP = 0` Stage 5 выбирает write-through: каждый успешный `write_sectors` имеет durable file-data ack до `Ok(())`. `Flush` остаётся поддержанным и всегда вызывает `NvmeBackend::flush`, в том числе при `VWC.VWCP = 0`. Это фиксирует политику host-интеграции и не меняет регистры, Identify или команды контроллера, которыми владеет `S7-16`.
- `attach_backend`: допустим только пока `CC.EN = 0` (`NvmeAttachError::ControllerEnabled`), backend с нулём секторов — `EmptyBackend`. `NSZE = NCAP = NUSE`, `NVMCAP`, `TNVMCAP` становятся `sector_count()` backend и заменяют `NvmeConfig::namespace_sectors`; Identify Namespace/Controller и проверка диапазона используют одно и то же значение. Заменить backend можно только при выключенном контроллере.
- **Решение: устройство существует и без backend.** До `attach_backend` контроллер работает (регистры, admin, Identify с размером из `NvmeConfig`), а любая `Read`/`Write`/`Flush` завершается `Namespace Not Ready` (после проверки NSID). Альтернатива — не создавать устройство без backend — отвергнута: `FirmwareBoardConfig::nvme` и handoff остаются как в срезе A, а владелец машины подключает диск позже.
- **Решение: один формат LBA, 512 байт** (`LBAF0`, `LBADS = 9`). 4 KiB LBA не поддержан и не объявляется (`NLBAF = 0`): 512 байт — единица `od-storage`, GPT S7-17 и размер, который ждут прошивка и `stornvme`.

### 13.4. Что не сделано (срез B)

| Что | Статус |
|---|---|
| LBA 4 KiB, несколько форматов LBA, Format NVM | не поддержаны (решение выше) |
| Команды `Dataset Management`, `Write Zeroes`, `Compare`, `Write Uncorrectable`, `Verify`, `Copy` | `Invalid Command Opcode`. Подсказки DSM в `Read`/`Write` принимаются независимо от `ONCS` |
| `PRINFO`, `STC` (защита данных), метаданные | формат LBA без метаданных и защиты данных. `PRINFO` и `STC` должны быть нулевыми |
| Namespace Management, несколько NS, широковещательный `NSID = 0xFFFF_FFFF` | вне scope; Flush на `0xFFFF_FFFF` отвергается |
| SGL | `PSDT ≠ 0` — `Invalid Field`, `SGLS = 0` |
| Файловый/CoW backend | вне `od-machine` (`od-storage`) |
| Прерывания | нет IRQ-маршрута, прошивка опрашивает CQ; `IEN` хранится без эффекта |
| PCI-привязка | S4-30 |
| Атомарность записи (`AWUN`/`AWUPF`), `Write Atomicity` | команда в `service` исполняется целиком; гарантии для гостя нет в обозначенных границах, поля не объявлены |
| Время исполнения, ограничение скорости | нет |
| I/O без созданной SQ | `Invalid Queue Identifier` невозможна: doorbell несозданной SQ даёт событие среза A (invalid doorbell register); `Invalid Queue Identifier` проверяется на Delete SQ несозданной очереди |

### 13.5. Открытые вопросы среза B (NOT VERIFIED)

1. Значения `LBA Out of Range` (`0x80`), `Namespace Not Ready` (`0x82`), `Internal Error` (`0x06`) сверены с `nvme.h`; `DNR` для них и условия применения — NOT VERIFIED против NVMe Base 2.4; для отказа backend спецификация, вероятно, предписывает медиа-статусы SCT 2.
2. Точный статус для ненулевых полей защиты данных в `Read`/`Write` при формате LBA без метаданных и защиты данных.
3. `Flush` с `NSID = 0` и поведение `Flush` без volatile write cache (`VWC.VWCP = 0`). Отказ для `NSID = 0xFFFF_FFFF` сверён с объявленным `VWC.FB = 10b`.
4. Порядок проверок (`Namespace Not Ready` после `NSID`, до полей), `MDTS`: статус при превышении (`Invalid Field`).
5. Единицы и округление `Data Units Read/Written` в SMART.
6. Обязательность `Write Zeroes`/`Dataset Management` для заявляемого профиля и требования `stornvme`/Linux к ним (первые кандидаты в дефекты после сверки).

## 14. Постоянный backend на стороне host (S5-02)

**Статус:** host contract S5-02 принят; host-side реализация `NvmeBackend` в S5-03 принята после независимого ревью кода и контрактов; проверенный исходный коммит S5-03 — `c20e3d6`. Этот раздел задаёт только host-side prerequisite для S5-03/S5-04. Он не принимает `S7-16`, не меняет его регистры/Identify/команды и не заменяет end-to-end guest storage/flush gate [S9-13](../plans/STAGE_9.md#s9-13-подключить-guest-storage-и-flush). Stage 9 владеет storage format/durability evidence (`STORAGE_FORMAT.md`), snapshots, capacity/explicit-zero/discard ([S9-18](../plans/STAGE_9.md#s9-18-capacity-explicit-zero-и-discard)) и power-loss testing ([S9-39](../plans/STAGE_9.md#s9-39-контролируемые-power-loss-испытания)); этап 5 не дублирует эти области.

### 14.1. Состав и владение

- `od-host` владеет открытием host-файла, единоличным write lock, проверкой его identity и преобразованием host/storage ошибок в `BackendError`. `od-storage` владеет проверяемым файловым API (`CheckedStorage`) и геометрией RAW (`RawImage`). `od-machine` владеет только `NvmeBackend` trait/controller; он не открывает файлы и не зависит от `od-host` или `od-storage`.
- Writable open использует существующий RAW regular file: `HostFileStorage::open_read_write_exclusive(path)` открывает его и немедленно берёт `flock(LOCK_EX | LOCK_NB)` на том же дескрипторе; второй writer получает `WriterBusy`, ожидания нет. Identity `(dev, ino)` проверяется после lock и до мутаций. Host open, lock, identity, длина/геометрия или durability-capability failure прекращают подготовку persistent VM до старта гостя; не подменять диск `InMemoryBackend` и не продолжать как будто он постоянный.
- Образ имеет тип `RawImage<HostFileStorage>`: `RawImage` хранит внутри `CheckedStorage<HostFileStorage>`, RAW не имеет header, а размер логического пространства равен размеру файла. `sector_count = logical_size / 512`; host adapter проверяет целые 512-byte сектора и checked LBA→byte arithmetic. Прочитанный `RawImage`/`CheckedStorage` не принимает файловый формат с заголовком.
- `HostFileStorage::open_read_only` не даёт lease и не защищает чтение от параллельного writer. Инструмент, которому нужно согласованное чтение, использует `ReadLease::open(path)` (`flock(LOCK_SH | LOCK_NB)`); shared lease несовместим с writable guest backend и остаётся отдельным reader path.

### 14.2. Write-through и durable completion

- При объявленном контроллером `VWC.VWCP = 0` каждая NVMe `Write` должна пройти: host adapter принимает полный backend buffer → проверяет диапазон/512-byte alignment → пишет его в `RawImage` → получает file-data durable acknowledgement из `CheckedStorage::flush_file_durable` → только затем возвращает `Ok(())` из `write_sectors`. Запись без успешного ack всегда возвращает ошибку; `RawImage::write_at` само означает только write completion и durability не подтверждает.
- Существующая композиция для S5-03: `RawImage::write_aligned` пишет через внутренний `CheckedStorage`; `RawImage::into_storage(self)` передаёт это checked-значение для `flush_file_durable()`. После успешного `DurableAck` host adapter заново создаёт RAW view через `RawImage::reopen_read_write_checked` на том же `CheckedStorage`, с неизменным logical size и block size 512. Этот narrow API сохраняет исходный writer lock и sticky state; при size/geometry validation error он возвращает тот же `CheckedStorage` вместе с `RawError`, чтобы terminal adapter мог удержать owner и не выпустить `LOCK_EX`. Не извлекать backend, не строить новый `CheckedStorage` и не сбрасывать sticky reconcile state при повторной обёртке. Повторное создание RAW view допускается только после успешного `DurableAck`. Если file flush после записи вернул `UnsupportedDurability` (включая `ENOTSUP`) или другую ошибку, операция не получает success, adapter переходит в terminal failed state, а host lifecycle помечает RAW backing quarantined; это не on-disk marker и не разрешает повторный path open. При недетерминированной ошибке `CheckedStorage` остаётся в sticky `ReconcileRequired`. Файл можно использовать снова только после отдельно определённой inspection/reconcile procedure, которая устанавливает состояние затронутых данных; такая процедура для RAW здесь не задаётся, и обычное повторное открытие recovery не является.
- `NvmeBackend::flush` всегда вызывает `CheckedStorage::flush_file_durable` и возвращает success только при `DurableAck`, даже при `VWC.VWCP = 0` или отсутствии ранее видимых writes. Отдельная guest `Flush` не является заменой per-Write barrier при `VWC.VWCP = 0`.
- `HostFileStorage` объявляет file durability только для writer с lock на macOS; там вызов `flush_file_durable` использует `fcntl(F_FULLFSYNC)` на этом дескрипторе. На не-macOS durability не объявляется, поэтому такая конфигурация не может присоединить persistent backend с `VWC.VWCP = 0`. Реальное поведение выбранного носителя и end-to-end `F_FULLFSYNC` не подтверждены испытаниями.
- Код отдельно классифицирует errno `ENOTSUP` как `UnsupportedDurability`, но это защитное предположение: источник не подтверждает этот errno для `F_FULLFSYNC`, фактическое появление ветки не проверено. Если capability/refusal известна до attach (`file_durability = false` либо explicit `UnsupportedDurability` при preflight до guest mutation), persistent VM не запускается: CQE нет, RAW не менялся гостем и не помещается в quarantine. Runtime NVMe `Flush` после attach всегда завершается CQE; explicit `UnsupportedDurability` до любой guest Write/mutation отображается в `Internal Error`, переводит adapter в terminal failed state для этой VM (новые Write/Flush запрещены), но RAW остаётся неизменным и не помещается в quarantine. Если explicit refusal или другая flush ошибка случилась после guest Write/mutation, file-data ack отсутствует, операция неуспешна, adapter terminally fails, RAW помечается host lifecycle как quarantined: возможные bytes нельзя считать сохранёнными, обычное переоткрытие не разрешено. Любой non-explicit flush failure до или после guest writes переводит `CheckedStorage` в sticky `ReconcileRequired`; runtime команда получает `Internal Error`, preflight прерывает запуск без CQE, а RAW остаётся недоступен до отдельной inspection/reconcile. Любой Write error, который мог оставить partial bytes, также переводит adapter в terminal failed state и помещает RAW в quarantine.
- Durability здесь касается только file contents. Namespace durability для create/rename/directory entry отсутствует (`flush_namespace_durable` → `unsupported`); создание или перемещение image не получает гарантию сохранения имени. Никакая из этих границ не заявляет сохранность при потере питания, kernel panic или отказе накопителя; эти испытания и claims принадлежат Stage 9/S9-39 ([STORAGE_FORMAT.md §7](STORAGE_FORMAT.md#7-host-durability-api--статус-частично-реализовано-не-подтверждено-испытаниями)).

### 14.3. Host failure → guest completion

| Событие | Host adapter | NVMe command result |
|---|---|---|
| Writer open/lock/identity/RAW geometry, `file_durability = false` либо explicit `UnsupportedDurability` обнаружен в preflight до attach/start | Ошибка подготовки persistent VM; не начинать guest execution и не подключать volatile substitute. Нет guest write; explicit refusal до mutation не quarantine image. | Нет CQE: backend ещё не подключён. Если тестовый/controller path намеренно работает без backend, существующая политика возвращает `Namespace Not Ready` (`SCT=0, SC=0x82`) на Read/Write/Flush. |
| Indeterminate file-flush failure во время preflight (не explicit UnsupportedDurability) | `CheckedStorage` становится sticky `ReconcileRequired`; abort startup и host-side quarantine. Ни guest bytes, ни CQE. | Нет CQE: команда не запускалась; ordinary reopen не разрешает image reuse. |
| Host Read failure после attach | Свернуть detail к `BackendError::Io`; не изменяет disk contents. | `Internal Error` (`SCT=0, SC=0x06, DNR=0`) по текущему generic mapping §13.2. |
| Runtime `Flush` error после attach, до любой guest Write/mutation | Свернуть detail к `BackendError::Io`; terminal failed adapter на эту VM, новые Write/Flush запрещены. Явный `UnsupportedDurability` не изменил RAW и сам по себе не quarantine; любой иной/indeterminate flush failure сохраняет sticky `ReconcileRequired` и quarantine. | CQE `Internal Error` (`SCT=0, SC=0x06, DNR=0`) для команды Flush — текущий generic mapping §13.2. |
| Host Write operation failure, который мог оставить partial bytes, либо file-flush failure после любой guest Write/mutation (включая `ENOTSUP`) | Свернуть detail к `BackendError::Io`; `write_sectors` не ack-ает без `DurableAck`. Terminal failed state + host-side quarantine; при indeterminate flush сохраняется sticky `ReconcileRequired`. | CQE `Internal Error` (`SCT=0, SC=0x06, DNR=0`) для Write/Flush — текущий generic mapping §13.2. Точная DNR/status-условность не сверена с NVMe Base 2.4; это документирует текущий controller contract, а не самостоятельное заявление о стандарте. |
| Host adapter получает некорректный sector range/length или `RawError::OutOfRange` | `BackendError::OutOfRange`; по текущему `NvmeBackend` contract это defense-in-depth, так как controller проверяет `SLBA/NLB/NSZE` до вызова backend | LBA-ошибка, обнаруженная самим controller — `LBA Out of Range` (`0/0x80`). Неожиданный `BackendError::OutOfRange` из backend идёт по общей ветке backend failure → `Internal Error` (`0/0x06`) согласно текущему §13.2. |
| Guest PRP или data transfer вне RAM | Обрабатывается до/при DMA стороне контроллера | Существующий `Data Transfer Error` (`SCT=0, SC=0x04`); это не код для host file failures. |
| Guest LBA выходит за `NSZE` до обращения к backend | Контроллер отвергает запрос до host I/O | Существующий `LBA Out of Range` (`SCT=0, SC=0x80`). Если сам backend неожиданно возвращает `BackendError::OutOfRange` после контроллерской проверки, текущая общая ветка backend errors всё равно даёт `Internal Error`; host error не меняет NVMe controller policy. |

Каждая runtime ошибка уходит гостю как CQE status; panic процесса, unwinding через boundary или ложный success запрещены. Значения и DNR остаются в области текущих оговорок §13.1–13.2; для отдельного media error (`Write Fault`/`Unrecovered Read Error`) источника или реализации нет, поэтому S5-02 их не выдумывает.

**Текущее evidence mapping:** [`nvme/io.rs`](../../crates/od-machine/src/nvme/io.rs) `check_common` возвращает `Namespace Not Ready` при отсутствующем backend, `check_transfer` возвращает `LBA Out of Range` до вызова backend, а Read/Write/Flush преобразуют любой backend `Err(_)` в `Status::INTERNAL_ERROR`; PRP/DMA status возвращается отдельным transfer path. Тесты [`nvme/tests.rs`](../../crates/od-machine/src/nvme/tests.rs) проверяют `BackendError::Io` → `(0, 0x06)`, отсутствующий backend → `(0, 0x82)` и guest transfer/range failures. Это evidence текущей реализации; статусные условия и DNR не сверены с NVMe Base 2.4 (§13.1–13.2).

### 14.4. Формат и границы

- S5-03/S5-04 используют один RAW image. CoW-format, snapshot metadata/chain, recovery и alternate disk formats не входят в Stage 5 host adapter; они остаются в Stage 9 (`STORAGE_FORMAT.md`, S9-08…22). Capacity/explicit-zero/discard остаются [S9-18](../plans/STAGE_9.md#s9-18-capacity-explicit-zero-и-discard), guest-level storage/flush acceptance — [S9-13](../plans/STAGE_9.md#s9-13-подключить-guest-storage-и-flush), controlled power-loss tests — [S9-39](../plans/STAGE_9.md#s9-39-контролируемые-power-loss-испытания). Stage 5 не копирует эти правила в NVMe layer.
- `NvmeHandle::attach_backend` вызывается только при `CC.EN = 0`; Attach при включённом контроллере остаётся `ControllerEnabled`. До старта гостевого исполнения host adapter проверяет nonzero 512-byte geometry, секторную ёмкость и file-durability capability. Контракт не меняет `VWC`, Identify Namespace/Controller, командный набор, LBA format или error status table, принадлежащие `S7-16`.
- `VWC.VWCP = 0` и per-Write ack не гарантируют atomicity многосекторной записи, guest filesystem consistency, namespace persistence или power-loss recovery. Indeterminate flush error либо любой failure после возможной guest mutation переводит backend в terminal failed state и RAW в quarantine до отдельно определённой inspection/reconcile; normal reopen или retry не доказывает, какие bytes durable. Explicit `UnsupportedDurability` до guest mutation тоже terminally fails adapter, но RAW остаётся unchanged и не quarantined. Это согласуется с неизвестным durability scope [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed). Отдельная строка `S9-13` проверяет сквозной guest-storage/flush contract; S5-02/S5-03 описывают и реализуют только host-side prerequisite.
