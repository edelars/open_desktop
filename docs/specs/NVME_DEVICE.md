# Минимальный NVMe-контроллер (S7-16)

**Статус: срезы A и B реализованы исполнителем, ожидают ревью.** Срез A — управляющая часть: регистры, admin-очереди и Admin-команды. Срез B — набор команд ввода-вывода `Read`, `Write`, `Flush` поверх backend диска (раздел 13; backend в памяти `InMemoryBackend`, файловый — вне `od-machine`). PCI-привязки нет (blocked до S4-30): устройство — фиксированное MMIO-окно, как у дисплея; прерываний нет (`gsi = 0xFFFF_FFFF`). Значения спецификации NVMe Base 2.4 не сверены (раздел 1).

Код: `crates/od-machine/src/nvme/{mod,controller,queues,admin,io,backend}.rs`, интеграция в плату — `crates/od-machine/src/firmware_board.rs`. Тесты — `nvme/tests.rs`, `nvme/queues.rs` (PRP), `firmware_board.rs` (handoff и `run_boot`).

## 1. Источники и статус сверки

Редакция по плану: NVM Express Base Specification 2.4, NVM Command Set Specification 1.3, NVMe over PCIe Transport Specification 1.4 (ratified 2026-07-31, [VIRTUAL_BOARD.md](VIRTUAL_BOARD.md)).

**Текст спецификаций не был получен.** Часть значений сверена с заголовком Linux `include/linux/nvme.h` (master, 2026-11-07), **не с текстом NVMe Base 2.4**. Это сверка с реализацией драйвера, а не со спецификацией. Что именно сделано 2026-11-07:

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

**NOT VERIFIED (ни спецификация, ни заголовок Linux не подтверждают):** `CAP.CQR` (бит 16); коды AER info 0 и 1; правила PRP; значения features по умолчанию; поведение при ошибках; обязательность команд; LID `0x00` и `0x05`; `VS` 2.0.0 против 2.4.0. В исходниках в шапках `controller.rs`, `admin.rs`, `queues.rs` оставлено то же разделение. Наименее надёжные места перечислены в разделе 11.

## 2. Размещение и handoff block

- **Окно MMIO:** `[0xFC00_1000, 0xFC00_3000)`, 8 KiB (`NVME_MMIO_BASE`, `NVME_MMIO_BYTES`). Регистры — `0x0000..0x1000`, doorbells — с `0x1000`, шаг 4 байта (`CAP.DSTRD = 0`). Выбранный диапазон лежит в зарезервированном «свободном» участке `[0xFC00_1000, 0xFEC0_0000)` ([FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) 2), сразу за регистрами дисплея, и не меняет число страниц `BOOT_PAGE_TABLES` (тот же блок 2 MiB). После S4-30 окно заменяет BAR0; адрес окна — решение v1, не требование спецификации NVMe.
- **Handoff block.** В v1.0 поля NVMe нет (коды 8–15 зарезервированы). Поэтому устройство добавляется как запись кода 8 и повышает `version_minor` до 1 ([FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) 6.3–6.4): `PRESENT = 1`, MMIO, `base = 0xFC00_1000`, `length = 0x2000`, `gsi = 0xFFFF_FFFF` (нет IRQ), `irq_flags = 0`, `param0 = param1 = 0`; в таблицу регионов добавляется регион `MMIO` с `owner_device = 7`.
- **Включается опционально:** `FirmwareBoardConfig::nvme: Option<NvmeConfig>`. По умолчанию `None`: handoff остаётся v1.0 байт в байт (эталон `REFERENCE_HANDOFF_HEX` не менялся), окна нет, `Unmapped`. При `Some` образ обязан объявлять `handoff_minor_max ≥ 1`, иначе загрузка отклоняется (`ImageHeader { field: "handoff_minor_max" }`). Прошивка в репозитории (`firmware/uefi`) пока понимает только minor 0 и NVMe-запись не читает — это работа S7-17.
- Устройство подключается к шине только когда запись PRESENT; без конфигурации оно не существует и не объявляется.
- **До PCI-привязки (S4-30) контроллер виден только прошивке** (по адресу из handoff block). `stornvme` и Linux `nvme` находят NVMe только по PCI class `010802`, поэтому ОС его не увидят; это блокирует S7-31 и S7-32.

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
| `0x0C` | `INTMS` (RW1S) | Маска pin-based прерываний (запись 1 ставит бит, чтение возвращает маску). Прерывание не доставляется (раздел 11), маска хранится |
| `0x10` | `INTMC` (RW1C) | Запись 1 снимает бит; чтение возвращает ту же маску |
| `0x14` | `CC` (RW) | Записываемые биты: `EN`, `CSS[6:4]`, `MPS[10:7]`, `AMS[13:11]`, `SHN[15:14]`, `IOSQES[19:16]`, `IOCQES[23:20]`. `CRIME` не реализован. Принимаются только `CSS = 0`, `MPS = 0`, `AMS = 0`, `IOSQES = 6` (SQE 64 байта), `IOCQES = 4` (CQE 16 байт) |
| `0x1C` | `CSTS` (RO) | `RDY` бит 0, `CFS` бит 1, `SHST[3:2]`; `NSSRO`, `PP` = 0 |
| `0x20` | `NSSR` | не реализован (`CAP.NSSRS = 0`), чтение 0, запись игнорируется |
| `0x24` | `AQA` | `ASQS[11:0]` и `ACQS[27:16]`, 0-based; допустимо 2…1024 записей |
| `0x28` | `ASQ` (64) | база admin SQ; биты `[11:0]` читаются как 0 |
| `0x30` | `ACQ` (64) | база admin CQ; биты `[11:0]` читаются как 0 |
| `0x1000+` | doorbells | раздел 5 |

**Включение и сброс.**

- `CC.EN` 0 → 1: проверяются `CSS/MPS/AMS/IOSQES/IOCQES` и размеры `AQA`. Верно — создаются admin SQ/CQ (пустые: `head = tail = 0`, фаза CQ = 1), `CSTS = RDY`. Неверно — `CSTS = CFS`, `RDY = 0`, очереди не создаются (поведение при неверных значениях — NOT VERIFIED, раздел 11). Выход из `CFS` — запись `CC.EN = 0`.
- `CC.EN` 1 → 0 — сброс контроллера: все очереди, features и невыполненные AER теряются, `CSTS = 0`. Error Information log и счётчики сохраняются.
- При `CC.EN = 1` меняется только `SHN`. Запись `SHN = 01b/10b` сразу выставляет `CSTS.SHST = 10b` (shutdown complete); состояния «в процессе» нет, так как нечего сбрасывать на носитель.
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
- Порядок: admin SQ первой, затем I/O SQ по номеру; внутри очереди — по порядку. Арбитраж round robin с burst 1 (приоритеты WRR не поддержаны).
- Ошибки команды — статус NVMe в CQE; `DNR = 1` для всех, кроме `Data Transfer Error` (повтор разрешён) — соглашение v1, NOT VERIFIED.
- **PRP.** Поддержаны `PRP1` со смещением (кратно 4 байтам), `PRP2` как вторая страница или как PRP list (в том числе со смещением внутри страницы), цепочка листов через последний элемент страницы; элементы листа и `PRP2`-страница обязаны быть выровнены по 4 KiB (`PRP Offset Invalid`, SC `0x13`); не более 1024 страниц на команду. Максимум передачи для I/O-команд: `MDTS = 8` (256 страниц, 1 MiB; одна страница PRP list вмещает 512 элементов, цепочка листов для этого размера не нужна). SGL не поддержан (`SGLS = 0`).

## 7. Admin-команды (срез A)

Мандаторные Admin-команды NVMe Base для контроллера ввода-вывода (по знанию авторов; обязательность NOT VERIFIED, сами opcode сверены с Linux `nvme.h`): `Delete I/O SQ`, `Create I/O SQ`, `Get Log Page`, `Delete I/O CQ`, `Create I/O CQ`, `Identify`, `Abort`, `Set Features`, `Get Features`, `Asynchronous Event Request`. Все десять реализованы. Любой другой opcode — `Invalid Command Opcode` (SC `0x01`, DNR), в том числе опциональные команды, которые контроллер не объявляет в `OACS`/`ONCS` (раздел 10).

| Opcode | Команда | Поведение и границы |
|---|---|---|
| `0x00` | Delete I/O SQ | `CDW10[15:0]` = QID. QID 0, больше 4 или не создана — `Invalid Queue Identifier` (SCT 1, SC `0x01`) |
| `0x01` | Create I/O SQ | QID — `1..=NSQA+1`, не занят. `CDW10[31:16]` = QSIZE (0-based), `CDW11`: `PC` (бит 0, обязан быть 1), `CQID[31:16]`. Порядок проверок: QID → CQID (нет такой CQ — `Completion Queue Invalid`, SCT 1, SC `0x00`) → размер (2…1024, иначе `Invalid Queue Size`, SC `0x02`) → `PC = 0` или `PRP1` не выровнен по странице (`Invalid Field`). `QPRIO` игнорируется (WRR нет). Область памяти очереди при создании не проверяется |
| `0x02` | Get Log Page | раздел 9 |
| `0x04` | Delete I/O CQ | как Delete SQ; если к CQ привязана SQ — `Invalid Queue Deletion` (SCT 1, SC `0x0C`) |
| `0x05` | Create I/O CQ | QID — `1..=NCQA+1`, не занят; `CDW11`: `PC` (обязан быть 1), `IEN` (бит 1), `IV[31:16]`. Порядок: QID → размер → `PC`/выравнивание → `IEN = 1` с `IV ≠ 0` — `Invalid Interrupt Vector` (SCT 1, SC `0x08`; при pin-based вектор один, 0). `IEN` сохраняется, прерывание не доставляется |
| `0x06` | Identify | раздел 8 |
| `0x08` | Abort | `CDW10`: `SQID[15:0]`, `CID[31:16]`. Несуществующая SQ — `Invalid Field`. Иначе успех с `DW0` бит 0 = 1 (команда не прервана): к моменту исполнения `Abort` все ранее извлечённые команды уже завершены (раздел 3). Лимит одновременных `Abort` объявлен `ACL = 3` и не может быть превышен. Прерывать нечего: команд в работе нет |
| `0x09` | Set Features | раздел 9 |
| `0x0A` | Get Features | раздел 9 |
| `0x0C` | Asynchronous Event Request | принимается и остаётся в очереди без completion, пока не произойдёт событие (раздел 9). Одновременно не более 4 (`AERL = 3`); пятый — `Asynchronous Event Request Limit Exceeded` (SCT 1, SC `0x05`). Сброс контроллера отбрасывает невыполненные AER |

Коды статусов (сверены с Linux `include/linux/nvme.h`, не с NVMe Base 2.4): `Invalid Command Opcode 0/0x01`, `Invalid Field in Command 0/0x02`, `Data Transfer Error 0/0x04`, `Invalid Namespace or Format 0/0x0B`, `Command Sequence Error 0/0x0C`, `PRP Offset Invalid 0/0x13`; command specific (SCT 1): `Completion Queue Invalid 0x00`, `Invalid Queue Identifier 0x01`, `Invalid Queue Size 0x02`, `Asynchronous Event Request Limit Exceeded 0x05`, `Invalid Interrupt Vector 0x08`, `Invalid Log Page 0x09`, `Invalid Queue Deletion 0x0C`.

## 8. Identify

`CDW10[7:0]` = CNS; результат — 4096 байт по PRP, остальное содержимое страницы — нули. Поддержаны:

| CNS | Структура | Границы |
|---|---|---|
| `0x00` | Identify Namespace | `NSID = 1`. Другое (0, 2…, `0xFFFF_FFFE`, `0xFFFF_FFFF`) — `Invalid Namespace or Format` |
| `0x01` | Identify Controller | раздел 8.1 |
| `0x02` | Список активных NSID | `[1]` при `NSID < 1`, иначе пустой (нули); `NSID ≥ 0xFFFF_FFFE` — `Invalid Namespace or Format` |
| `0x03` | Список Namespace Identification Descriptor | `NSID = 1`: дескриптор `NIDT = 2` (NGUID, 16 байт), затем `NIDT = 4` (CSI = 0, NVM command set), затем нулевой терминатор |

Остальные CNS (в том числе I/O-command-set-specific `0x05/0x06/0x1C…`, NVM Set, secondary controller, allocated namespace list) — `Invalid Field in Command`.

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
| 261 | `LPA` | 0 | SMART и Firmware Slot log — на весь контроллер; нет Commands Supported and Effects, телеметрии |
| 262 | `ELPE` | 63 | 64 записи Error Information log |
| 263 | `NPSS` | 0 | одно состояние питания |
| 266, 268 | `WCTEMP`, `CCTEMP` | 343, 358 K | пороги предупреждения и критический |
| 280 | `TNVMCAP` (16) | `namespace_sectors · 512` | `UNVMCAP = 0` (нет Namespace Management) |
| 512 | `SQES` | `0x66` | только SQE 64 байта |
| 513 | `CQES` | `0x44` | только CQE 16 байт |
| 516 | `NN` | 1 | |
| 520 | `ONCS` | 0 | необязательных NVM-команд нет: `Read`/`Write`/`Flush` в `ONCS` не объявляются (NOT VERIFIED: какие команды обязательны); Compare, Write Uncorrectable, Dataset Management, Write Zeroes, Verify, Copy — не реализованы |
| 522 | `FUSES` | 0 | fused-операций нет |
| 525 | `VWC` | 0 | энергозависимого кэша записи в контроллере нет; `Flush` всё равно вызывает `flush` backend (раздел 13) |
| 536 | `SGLS` | 0 | SGL не поддержан |
| 768 | `SUBNQN` | `nqn.2014-08.org.nvmexpress:uuid:<UUID>` | UUID из `NvmeConfig::subsystem_uuid`, дополнено нулями |
| 2048 | `PSD0` | `MP = 2500` (25,00 Вт), задержки 0 | описание единственного состояния питания |

### 8.2. Identify Namespace (NSID 1)

`NSZE = NCAP = NUSE = namespace_sectors` (поддержки thin provisioning нет); `NSFEAT = 0`; `NLBAF = 0` (один формат LBA); `FLBAS = 0`; `NVMCAP = namespace_sectors · 512`; `NGUID = namespace_uuid`; `EUI64 = 0`; `LBAF0`: `MS = 0`, `LBADS = 9` (512 байт), `RP = 0`. **Решение о 4 KiB LBA:** в v1 формат один, 512 байт; формат 4096 не объявляется (сектор 512 байт — единица `od-storage` и размер, который принимают `stornvme`, прошивка и GPT S7-17). Расширение — отдельное решение до среза B.

## 9. Features и log pages

### 9.1. Features

`CDW10[7:0]` = FID; Get — `CDW10[10:8]` = SEL: поддержаны `0` (текущее) и `1` (значение по умолчанию); `2` (сохранённое) и `3` (возможности) — `Invalid Field`. Set — бит `SV` (31): сохранение не поддержано (`ONCS` бит 4 = 0), `SV = 1` — `Invalid Field`. Features сбрасываются к значениям по умолчанию сбросом контроллера. Неподдержанный FID — `Invalid Field`.

| FID | Feature | Поддержка |
|---|---|---|
| `0x01` | Arbitration | хранится (`AB`, `LPW/MPW/HPW`; арбитраж фактически round robin); по умолчанию `AB = 7` |
| `0x02` | Power Management | только `PS = 0`, без workload hint; иное — `Invalid Field` |
| `0x04` | Temperature Threshold | датчик 0 (composite): порог over (`THSEL = 0`, по умолчанию 343) и under (`THSEL = 1`, по умолчанию 0); `TMPSEL ≠ 0` или `THSEL > 1` — `Invalid Field`; события по порогам не генерируются (температура постоянная) |
| `0x05` | Error Recovery | `TLER[15:0]`, `DULBE` не поддержан (`Invalid Field`); `NSID` — 0, 1 или `0xFFFF_FFFF`, иначе `Invalid Namespace or Format` |
| `0x07` | Number of Queues | Get — текущее число (по умолчанию 4 SQ и 4 CQ, 0-based `3`, `DW0 = NSQA \| NCQA << 16`); Set — `NSQR/NCQR`, значение больше доступного усекается, `0xFFFF` — `Invalid Field`; после создания любой I/O-очереди — `Command Sequence Error` |
| `0x08` | Interrupt Coalescing | `THR/TIME` хранятся, на доставку не влияют |
| `0x09` | Interrupt Vector Configuration | только `IV = 0`, `CD` хранится; иное — `Invalid Field` |
| `0x0A` | Write Atomicity Normal | `DN` хранится |
| `0x0B` | Asynchronous Event Configuration | допустимы только биты `[7:0]` (SMART/Health critical warning); `OAES = 0` |

Не поддержаны, `Invalid Field`: `0x03` LBA Range Type, `0x06` Volatile Write Cache (`VWC = 0`), `0x0C` APST, `0x0D…0x0F`, `0x80` Software Progress Marker и все остальные.

### 9.2. Get Log Page

`CDW10`: `LID[7:0]`, `RAE` (бит 15), `NUMDL[31:16]`; `CDW11[15:0]` = `NUMDU`; смещение `LPO` — `CDW12/CDW13`, обязано быть кратно 4. Читается `(NUMD + 1) · 4` байт с `LPO`; запрос за пределами страницы — `Invalid Field`. `NSID` для `0x02` и `0x03` — 0 или `0xFFFF_FFFF` (`LPA` бит 0 = 0), иначе `Invalid Field`. Неизвестный `LID` — `Invalid Log Page` (SCT 1, SC `0x09`). Содержимое — настоящее состояние, не нули-заглушки там, где поле имеет смысл:

| LID | Страница | Содержимое |
|---|---|---|
| `0x01` | Error Information | до 64 записей по 64 байта, новейшая первой. Заводится на каждую команду с ненулевым статусом (кроме событий-doorbell, у них свои записи): `Error Count` (с 1, растёт), `SQID`, `CID`, `Status Field` (`P` из CQE, `SC`, `SCT`, `DNR`), `Parameter Error Location = 0xFFFF` (не отслеживается), `NSID` команды (`0xFFFF_FFFF`, если 0). Неиспользованные записи — нули (`Error Count = 0`) |
| `0x02` | SMART / Health Information (512) | `Critical Warning = 0`; `Composite Temperature = 300` K; `Available Spare = 100`, `Threshold = 10`; `Percentage Used = 0`; `Data Units Read/Written` (смещения 32/48) — тысячи секторов по 512 байт, с округлением вверх, `Host Read/Write Commands` (64/80) — число успешных `Read`/`Write` с включения питания (срез B; NOT VERIFIED: единицы и округление); `Controller Busy Time` = 0 (нет времени); `Power Cycles = 1`; `Number of Error Information Log Entries` — живой счётчик ошибок; остальное 0. **`Power On Hours`, `Controller Busy Time` = 0, так как у устройства нет источника времени** — разрыв |
| `0x03` | Firmware Slot Information (512) | `AFI = 0x01` (активен слот 1, следующий — нет); `FRS1` = `FR`; слоты 2–7 пусты (нули) |

Чтение с `RAE = 0` снимает маску асинхронного события соответствующего типа (`0x01` — тип 0 «error status», `0x02` — тип 1).

### 9.3. Асинхронные события

События генерирует только контроллер при ошибочных doorbell (раздел 5): тип 0 (error status), `info` 0 или 1, log page `0x01`. `DW0` completion AER = `тип \| info << 8 \| log_page << 16`. Событие, возникшее без невыполненного AER, ждёт и завершает следующий AER сразу. Пока событие данного типа не «прочитано» (`Get Log Page` с `RAE = 0`), новые события того же типа не передаются (запись в Error Information log всё равно добавляется). Событий SMART/NS-изменений/firmware-активации нет.

## 10. Что не сделано и почему

| Что | Причина и статус |
|---|---|
| I/O `Read`, `Write`, `Flush` | реализованы в срезе B (раздел 13). Остальные NVM-команды (`Write Zeroes`, `Dataset Management`, `Compare`, `Write Uncorrectable`, `Verify`, `Copy`) — `Invalid Command Opcode`; обязательность и состав набора для заявляемой редакции не сверены (раздел 11) |
| Файловый backend / CoW-образ | `od-machine` содержит только trait `NvmeBackend` и `InMemoryBackend` (раздел 13); подключение `od-storage` — за владельцем машины |
| Namespace Management/Attachment, несколько NS | вне scope S7-16; один NS `NSID = 1`, `OACS` бит 3 = 0 |
| Firmware Image Download/Commit | вне scope; `FRMW` объявляет слот 1 только для чтения |
| Device Self-test, Format NVM, Sanitize, Security Send/Receive, Keep Alive, Directives, Virtualization, Doorbell Buffer Config | вне scope; `Invalid Command Opcode` |
| SGL | вне scope; `PSDT ≠ 0` — `Invalid Field`, `SGLS = 0` |
| Доставка прерываний | нет IRQ-маршрута (`gsi = 0xFFFF_FFFF`); `INTMS/INTMC`, `IEN`, `Interrupt Coalescing` хранятся без эффекта; прошивка опрашивает CQ. Pin-based через IOAPIC — отдельная часть S7-16; MSI-X — после S4-30 |
| PCI-привязка (class `010802`, BAR0, MSI-X) | blocked до S4-30; до неё контроллер виден только прошивке |
| Supported Log Pages (`LID 0x00`), Commands Supported and Effects (`0x05`), Changed Namespace List, телеметрия | не реализованы; обязательность `0x00` и `0x05` для заявляемой редакции не сверена (раздел 11). `Invalid Log Page` |
| Время (`Power On Hours`, `Controller Busy Time`, `TO`) | у устройства нет виртуальных часов; `RDY` выставляется синхронно, объявлено `TO = 1` |
| `NSSR` (NVM subsystem reset), `CRIME`, CMB, PMR, boot partitions, WRR, ключевые события температуры | не реализованы; `CAP` не объявляет их |
| Идентификаторы `VID`, `SSVID`, `IEEE` | нулевые; значения проекту не выданы (раздел 8.1) |
| Моделирование времени исполнения команд | команды завершаются в `service`, без виртуальной задержки |

## 11. NOT VERIFIED: места, где расхождение наиболее вероятно

Сверка с Linux `nvme.h` (раздел 1) не заменяет сверку с текстом NVMe Base 2.4; NOT VERIFIED остаётся всё, что в разделе 1 не перечислено как сверенное. Особо проверить при сверке:

- положение и значения полей `CAP` (`MQES`, `CQR`, `TO`, `CSS` бит 37, `MPSMIN/MPSMAX`) и значение `VS` для заявляемой редакции (2.0.0 против 2.4.0; согласовать с `Identify Controller.VER`);
- **вероятное отклонение:** `CSTS.CFS = 1` при `CC.EN = 1` с `IOSQES ≠ 6` или `IOCQES ≠ 4` (`controller.rs`, `enable`). По спецификации (неподтверждённо) эти поля проверяются при создании I/O-очереди (Create I/O SQ/CQ), а не при включении контроллера; поведение не менялось и не проверялось (`NOT VERIFIED against NVMe Base 2.4: проверка IOSQES/IOCQES при EN=1 против при Create I/O SQ/CQ`);
- битовые поля `CC`, `CSTS`, `AQA` и поведение при недопустимых значениях `CC` (здесь — `CSTS.CFS = 1`, `RDY = 0`) и при записи `AQA/ASQ/ACQ` при `CC.EN = 1` (здесь — `Busy` на шине, без эффекта);
- смещения и размеры полей Identify Controller / Namespace, `FRMW`, `LPA`, `SQES/CQES`, `SUBNQN`, `PSD0`, значение `CNTRLTYPE`, нужны ли `CNS 0x03` и I/O-command-set-specific CNS для заявляемой редакции;
- перечень обязательных Admin-команд и обязательных features для контроллера ввода-вывода PCIe (раздел 7, 9.1), значения по умолчанию features (`Arbitration = 7`, порог 343 K), допустимость `SEL ≠ 0`;
- коды статусов и их SCT, особенно `PRP Offset Invalid`, `Invalid Interrupt Vector`, `Invalid Queue Deletion`, порядок проверок при создании очередей, `DNR`;
- формат Error Information log (`Status Field`, `Parameter Error Location`), какие команды в него попадают, поведение при чтении за пределами страницы, обязательность `LID 0x00` и `0x05`;
- коды событий AER (тип 0, `info` 0 и 1) и правила маскирования до `Get Log Page` с `RAE = 0`;
- ответ `Abort`, когда цель не найдена (`DW0` бит 0 = 1), и поведение при несуществующей SQ;
- реакция на doorbell несозданной очереди и на значение вне диапазона (здесь — запись принимается, событие и запись в журнал, а не ошибка шины);
- правила PRP: допустимое смещение `PRP1` и `PRP2`-листа, цепочка листов.

**Открытые вопросы среза B (для ревью и S7-17):** см. раздел 13.5.

**Открытые вопросы среза A (для ревью и S7-17):**

1. Проверка `IOSQES`/`IOCQES` при `CC.EN = 1` против проверки при Create I/O SQ/CQ (выше, вероятное отклонение).
2. Supported Log Pages (`LID 0x00`) и Commands Supported and Effects (`LID 0x05`) не реализованы (`Invalid Log Page`); обязательность для заявляемой редакции не сверена — первый кандидат в дефекты.
3. `VS = 2.0.0` (и `Identify Controller.VER`) против 2.4.0.
4. Запись doorbell гостем не покрыта тестом end-to-end (раздел 3).
5. `CAP.CQR`, коды AER info, правила PRP, умолчания features, поведение при ошибках — NOT VERIFIED (раздел 1).

## 12. Проверка

`cargo test -p od-machine --offline` — модуль `nvme` (регистры, ошибки доступа, `CC.EN/RDY/CFS`, Identify побайтно, active NS list, создание/удаление очередей с фазовым битом и заполненной CQ, ошибка создания SQ без CQ, Features, три log page, AER, Abort, неверный doorbell, неподдержанная команда, PRP на границе страницы и со списком) и `firmware_board` (handoff v1.1 с NVMe, окно не отображено без устройства, `run_boot` выполняет заранее поданные команды; путь «гость пишет doorbell → остановка кванта → `service`» end-to-end не покрыт, раздел 3). Срез B: тесты `nvme/tests.rs` (раздел «I/O») и `nvme/backend.rs`, `nvme/queues.rs` (`read_from_host`): Write → Read через PRP1+PRP2 и PRP list, максимум 1 MiB, Flush, LBA/NSID/NLB/поля/PRP-ошибки, ошибка backend, нет backend, полная CQ, отсутствующая SQ, счётчики SMART. Тесты, зависящие от значений раздела 1, проверяют только то, что объявлено здесь; они не доказывают соответствие спецификации (сверка — с Linux `nvme.h`, раздел 1).

## 13. Срез B: I/O

Код: `nvme/io.rs` (команды), `nvme/backend.rs` (trait и `InMemoryBackend`), `nvme/controller.rs` (`attach_backend`, исполнение команд очереди I/O). I/O-команды берутся из SQ 1…4 тем же циклом `service`, что и admin (раздел 6): порядок, фазовый бит, полная CQ, `SQHD` — без изменений.

### 13.1. Команды

Opcode `Flush = 0x00`, `Write = 0x01`, `Read = 0x02` сверены с Linux `include/linux/nvme.h` (master, 2026-11-07), не с NVMe Base 2.4 / NVM Command Set 1.3. Значения статусов раздела 13.2 (включая `LBA Out of Range` `0x80`, `Namespace Not Ready` `0x82`, `Internal Error` `0x06`) и бит DNR (`0x4000`) тоже сверены с `nvme.h`. **Всё остальное в этом разделе — `NOT VERIFIED against NVMe Base 2.4`:** раскладка полей `Read`/`Write`, DNR для статусов и условия их применения, порядок проверок, поведение `Flush`.

| Поле | Read / Write |
|---|---|
| `NSID` | только 1 |
| `PRP1`, `PRP2` | данные; правила PRP — раздел 6 |
| `CDW10`, `CDW11` | `SLBA` (младшие, старшие 32 бита) |
| `CDW12[15:0]` | `NLB`, 0-based: 1…2048 секторов (512 байт … 1 MiB, `MDTS = 8`) |
| `CDW12[31:16]` | `FUA` (бит 30), `LR` (31), `PRINFO` (26–29), `DTYPE` (20–23) и прочее — не поддержаны: любое ненулевое значение — `Invalid Field` |
| `CDW13` | подсказки Dataset Management (`DSM`) — не поддержаны: ненулевое — `Invalid Field` |
| `CDW14`, `CDW15`, `MPTR` | игнорируются: защиты данных (`PRINFO = 0`) и метаданных (`MS = 0`) нет |

`Flush`: `NSID = 1`; `PRP`, `CDW10..15` игнорируются. `NSID = 0xFFFF_FFFF` (все пространства имён) не принимается: `Invalid Namespace or Format`.

**Порядок проверок** (первая ошибка — статус команды): opcode (`Invalid Command Opcode`) → `FUSE ≠ 0` или `PSDT ≠ 0` (`Invalid Field`) → `NSID ≠ 1` (`Invalid Namespace or Format`) → backend не подключён (`Namespace Not Ready`) → (Read/Write) `CDW12[31:16]`/`CDW13 ≠ 0` (`Invalid Field`) → `NLB + 1 > 2048` (`Invalid Field`) → `SLBA + NLB + 1 > NSZE` с проверкой переполнения 64 бит (`LBA Out of Range`) → передача.

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
- `InMemoryBackend` (`new(sectors)`, `from_bytes`, `as_bytes`, `flush_count`) — нулевой вектор фиксированного размера для тестов и мини-образов. **Точка интеграции файла/CoW:** владелец машины реализует `NvmeBackend` над контрактами `od-storage` (`STORAGE_FORMAT.md`) и передаёт его в `NvmeHandle::attach_backend(Box<dyn NvmeBackend + Send>)`. Файлового ввода-вывода в `od-machine` нет. **Требование долговечности:** контроллер объявляет `VWC = 0`, драйверы Flush не посылают, поэтому backend обязан сделать запись долговечной к возврату `write_sectors` (write-through), либо интеграция должна объявить `VWC = 1` и реализовать Flush; выбор — открытый вопрос для интеграции с `od-storage`. Поведение кода не менялось.
- `attach_backend`: допустим только пока `CC.EN = 0` (`NvmeAttachError::ControllerEnabled`), backend с нулём секторов — `EmptyBackend`. `NSZE = NCAP = NUSE`, `NVMCAP`, `TNVMCAP` становятся `sector_count()` backend и заменяют `NvmeConfig::namespace_sectors`; Identify Namespace/Controller и проверка диапазона используют одно и то же значение. Заменить backend можно только при выключенном контроллере.
- **Решение: устройство существует и без backend.** До `attach_backend` контроллер работает (регистры, admin, Identify с размером из `NvmeConfig`), а любая `Read`/`Write`/`Flush` завершается `Namespace Not Ready` (после проверки NSID). Альтернатива — не создавать устройство без backend — отвергнута: `FirmwareBoardConfig::nvme` и handoff остаются как в срезе A, а владелец машины подключает диск позже.
- **Решение: один формат LBA, 512 байт** (`LBAF0`, `LBADS = 9`). 4 KiB LBA не поддержан и не объявляется (`NLBAF = 0`): 512 байт — единица `od-storage`, GPT S7-17 и размер, который ждут прошивка и `stornvme`.

### 13.4. Что не сделано (срез B)

| Что | Статус |
|---|---|
| LBA 4 KiB, несколько форматов LBA, Format NVM | не поддержаны (решение выше) |
| `Dataset Management` (команда и подсказки `CDW13`), `Write Zeroes`, `Compare`, `Write Uncorrectable`, `Verify`, `Copy` | `Invalid Command Opcode` / `Invalid Field`; `ONCS = 0` |
| `FUA`, `LR`, `PRINFO` (защита данных), метаданные | `Invalid Field`; не объявлены |
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
2. Раскладка `CDW12`/`CDW13` Read/Write (позиции `FUA`, `LR`, `PRINFO`, `DTYPE`) и реакция на их ненулевые значения; нужен ли `Invalid Field` для `CDW14/15` при `PRINFO = 0`.
3. `Flush` с `NSID = 0xFFFF_FFFF` и с `NSID = 0`; поведение `Flush` без volatile write cache (`VWC = 0`).
4. Порядок проверок (`Namespace Not Ready` после `NSID`, до полей), `MDTS`: статус при превышении (`Invalid Field`).
5. Единицы и округление `Data Units Read/Written` в SMART.
6. Обязательность `Write Zeroes`/`Dataset Management` для заявляемого профиля и требования `stornvme`/Linux к ним (первые кандидаты в дефекты после сверки).
