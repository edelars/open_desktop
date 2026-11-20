# Контракт persistent форматов хранения

Статус: **S9-00 принято с оговорками по архитектурному ревью; формат CoW v1 принят владельцем проекта как нормативный 2026-11-06 (решение пользователя; см. §9)**. Документ фиксирует scope, границы, модель отказов, лимиты и правила подтверждения. Нормативные bytes CoW v1 — в §9 (root slot, карта, лимиты, протокол коммита, create). Слой snapshot-метаданных (ODSNAP) нормативно не описан и не входит в принятие; RAW — без header по §1. Реализованы (коммиты `a59338a`, `ba77773`, `4fcd5c4`, `69e0b02`): контракт [`io.rs`](../../crates/od-storage/src/io.rs) (`StorageBackend`, `CheckedStorage`), [`raw.rs`](../../crates/od-storage/src/raw.rs) (`RawImage`), host backend с `flock` и flush данных файла в [`storage_io.rs`](../../crates/od-host/src/storage_io.rs). Есть прототип CoW (S9-08…12; коммиты `000c1da`, `17c9ce6`, `61a99e0`, `7d23bf9`, `01ef35d`): [`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), [`cow_parse.rs`](../../crates/od-storage/src/cow_parse.rs), [`cow_tx.rs`](../../crates/od-storage/src/cow_tx.rs) с тестами на fault backend ([`testing/fault.rs`](../../crates/od-storage/src/testing/fault.rs), `a7874b8`); CoW v1 нормативен (§9); S9-40 не завершён (commit-latency: [S9_40_COMMIT_LATENCY.md](../testing/S9_40_COMMIT_LATENCY.md); throughput: [S9_40_THROUGHPUT.md](../testing/S9_40_THROUGHPUT.md)); [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) закрыт (`01ef35d`). Snapshot metadata — прототип S9-14. Full-VM container — кодек S9-23 реализован локально (§13), без данных участников, не принят. Namespace durability не реализована, power-loss не проверен.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [этап 9 roadmap](../ROADMAP.md#этап-9-надёжность-данных-и-управление-состоянием), [архитектура §7](../ARCHITECTURE.md#7-диски-конфигурация-и-снимки), [виртуальная плата](VIRTUAL_BOARD.md), [матрица поддержки](SUPPORT_MATRIX.md), [состояние VM и операции](VM_STATE.md), [методика recovery](../testing/STORAGE_RECOVERY.md).

## 1. Область документа

Документ описывает долговечные bytes, которые Open Desktop пишет на host storage и потом читает обратно:

| Artifact | Владелец кода | Статус формата |
|---|---|---|
| RAW disk image | `crates/od-storage/src/` | Без header: ровно `logical_size` bytes guest LBA space. Семантика в §4–§7; реализован `RawImage` ([`raw.rs`](../../crates/od-storage/src/raw.rs), `4fcd5c4`), ревью S9-05 пройдено, power-loss не проверен |
| Собственный CoW disk image | `crates/od-storage/src/` | **CoW v1 — нормативен (принято владельцем 2026-11-06)**: layout и правила — §9; реализация [`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), [`cow_parse.rs`](../../crates/od-storage/src/cow_parse.rs), [`cow_tx.rs`](../../crates/od-storage/src/cow_tx.rs); S9-40 (throughput, чтение, WA, recovery, память) остаётся незавершённым и не отменяется этим решением |
| Snapshot metadata и цепочки | `crates/od-storage/src/` | Прототип (S9-14…16, `7188582`, `33440be`): [`cow_snapshot.rs`](../../crates/od-storage/src/cow_snapshot.rs), `MAX_CHAIN_DEPTH = 8`; не нормативен (§9.8), в отличие от CoW v1; resolver цепочки только в офлайн-CLI image-tool (`PathResolver`, `1590c9e`), VMHost цепочку не открывает |
| Full-VM checkpoint container и manifest | container — `od-storage`, состав участников — `od-machine` | Кодек контейнера реализован локально (S9-23, не нормативен, без данных участников): §13; список участников по профилям — [VM_STATE.md §5](VM_STATE.md#5-участники-контейнера-по-профилям-s9-23) |
| UEFI variable store artifact | семантика — `firmware/uefi/src/`, банковый снимок — `od-machine`, persistence — `od-storage` | Доступен `ODNVBANK` v1/v2. Контейнер и запись в bundle относятся к S9-26. [Контракт NVRAM](NVRAM_DEVICE.md#host-api) |
| vTPM persistent state | состояние и кодек — `od-tpm`, доступ к байтам платы — `od-machine`, persistence — `od-storage`, защита ключей — `od-host` | Доступен `ODTPMSTA` v6 с чтением v3–v5 для текущего профиля. Защищённый контейнер относится к S9-27. [Контракт TPM](VTPM.md#host-api-и-persistence) |

Вне документа:

- Межпроцессные и гостевые wire formats — нормативно в `protocols/` (например, [control v1](../../protocols/control/CONTROL_PROTOCOL_V1.md)); здесь на них только ссылки.
- C ABI — `include/open_desktop/` и [HOST_SESSION_ABI_V1.md](HOST_SESSION_ABI_V1.md).
- UI-каталог машин — восстанавливаемый индекс, не источник истины ([архитектура §7](../ARCHITECTURE.md#каталог-vm)).
- JIT cache, derived TLB, native pointers, host file descriptors и kernel handles никогда не являются persistent полями.
- Rust layout, `repr(C)` и формат `serde` по умолчанию не задают долговечный формат.

## 2. Профили

| Профиль | Состав | Требует работающей VM | Статус |
|---|---|---|---|
| `offline-raw-v0` | RAW image, один writer, явный durable flush, reopen | Нет | Целевой первый срез S9-03…07; реализованы S9-03…05 и часть S9-04 (writer lock, read lease)/S9-06 (file-data flush); fault backend S9-07 и namespace durability отсутствуют; профиль не принят |
| `offline-cow-v1` | Собственный CoW, recovery, immutable snapshots, check/export/salvage/migration | Нет | S9-08…12 реализованы на формате CoW v1 (нормативен с 2026-11-06; S9-40 не завершён; ISSUE-057 закрыт, `01ef35d`); S9-14…21 реализованы (snapshots, branch, проверка цепочки, удаление и offline compaction, capacity/zero/discard, read-only inspect/check CLI, RAW export/import и salvage; S9-20/21 без независимого ревью); S9-22 частично (замороженная фикстура и тесты чтения, `b5f75d3`; migration нет). S9-13 (guest storage/flush) относится к VM-профилям |
| `restricted-vm` | Тестовая машина без UEFI/vTPM: CPU/RAM/devices/time/I/O + диски | Да | Отдельный ограниченный gate; не закрывает Windows gate (план §2, §6) |
| `full-windows-vm` | Всё из `restricted-vm` + UEFI variables + vTPM + identity policy | Да | Целевой; заблокирован отсутствующими компонентами, см. [VM_STATE.md](VM_STATE.md#2-инвентаризация-prerequisites-s9-01) |

Offline-профили не зависят от CPU, Windows или vTPM. Ни один профиль не принят.

## 3. Первая host/filesystem/media комбинация

| Параметр | Кандидат первого цикла | Основание | Статус |
|---|---|---|---|
| Host OS | macOS Tahoe 26.7.1, build 25G241 | [SUPPORT_MATRIX](SUPPORT_MATRIX.md) | Предложено |
| Машина | MacBook Pro, Apple M3 Max (локальный хост) | [SUPPORT_MATRIX](SUPPORT_MATRIX.md); точная модель/год накопителя не зафиксированы | Предложено |
| Filesystem | APFS, data volume внутреннего контейнера | Read-only `diskutil info /` 2026-11-05: APFS, internal, fixed, solid state, protocol Apple Fabric, FileVault on | Предложено; не испытано |
| Media | Внутренний SSD Mac | То же | Модель контроллера и cache policy не зафиксированы |
| Durability API | `F_FULLFSYNC` для данных файла (macOS); namespace — не реализовано | См. §7 | **Открыто**: не испытано, namespace отсутствует ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)) |

Явно **не получают** гарантий этой комбинации без отдельного S9-06/S9-39 evidence: сетевые filesystems (SMB/NFS/AFP), File Provider/облачные каталоги, внешние и removable накопители, другие filesystems (HFS+, exFAT, FAT), образы внутри disk images/sparsebundle, Linux ext4/xfs на Mini. Linux Mini (`senya@192.168.1.14`, [STAND.md](../testing/STAND.md)) используется для переносимости bytes и сравнения, но не как oracle гарантий APFS/macOS. Open попытка на неподдержанном backend должна либо отказать в durable операциях, либо работать в явно помеченном non-durable режиме; молчаливое наследование гарантий запрещено.

## 4. Единицы, размеры и лимиты

Все значения ниже — **параметры первого профиля**. Значения на горячем пути (размер CoW block, структура map) — параметры v1 (приняты 2026-11-06); их производительность — в S9-40. Изменение значения не меняет правила проверки.

| Параметр | Правило | Начальное значение | Статус |
|---|---|---|---|
| Guest logical block (LBA) size | Фиксируется при create, не меняется; все guest-visible смещения кратны ему | 512 B: NVMe-контроллер S7-16 ([NVME_DEVICE.md §13.3](NVME_DEVICE.md#133-backend-и-namespace)) объявляет один формат LBA в 512 B; образ этапа 9 гостю ещё не подключён | Выбрано для NVMe-профиля; применение к образу — S9-13 |
| Logical size RAW | Кратен LBA size; `> 0`; checked arithmetic до любого I/O | В [`raw.rs`](../../crates/od-storage/src/raw.rs) проверяются `size > 0`, block size — степень двойки и кратность; явного верхнего лимита профиля нет, действует только `max_len` backend (`StorageCapabilities`). S9-05 лимит не задал | Открытый вопрос; владелец решения — S9-00 (границы профиля), применение в `RawImage` — S9-05 |
| CoW block size | Степень двойки, кратна LBA size, фиксирована на image | 512 Б (`BLOCK_SIZE`) | Нормативно в CoW v1 (§9); замеры S9-40 не дали основания менять |
| Virtual capacity CoW | Явный maximum в header; map bounded | `MAX_BLOCKS = 1 << 16` блоков, `MAX_FILE_BYTES = 1 << 32` (§9.3) | Нормативно в v1 |
| Глубина snapshot chain | Явный maximum; превышение — отказ open/create, не обход | S9-14 | `MAX_CHAIN_DEPTH = 8` (параметр snapshot-слоя, не нормативен в v1) |
| Число snapshots/roots | Явный maximum | S9-14: `MAX_SNAPSHOTS = 8`; roots v1 — два slot (§9) | Snapshots — параметр snapshot-слоя, не нормативен (§9.8) |
| Размер metadata structures | Каждая длина проверяется против лимита до allocation/read | `MAX_MAP_BYTES = MAX_BLOCKS × 12`, корень 512 Б (§9.2–9.3) | Нормативно в v1 |

Guest page size (4 KiB) не равен host page size (16 KiB на основном Mac); формат и I/O не опираются на их равенство.

Правила для любого будущего формата (S9-08, S9-14, S9-23):

- Явные endianness, длины, version, feature bits (mandatory/optional), logical и physical sizes, generation/transaction IDs, object/parent identity, checksums и bounds.
- Unknown mandatory feature запрещает writable open; readonly open — только при доказанно понятной семантике.
- Все parser limits объявлены в спецификации; overflow проверяется до allocation и чтения.
- Rust ABI и host pointers в формат не входят.

## 5. Single-writer policy

- Один writable backing (RAW file, CoW image, persistent artifact) имеет ровно одного writer — одну VMHost или один offline tool — в каждый момент.
- Enforcement межпроцессный (S9-04, host primitive в `od-host`); Rust ownership внутри процесса недостаточна. Path string и PID-файл не являются identity: нужна стабильная file/object identity с учётом same-file aliases на поддержанной платформе.
- Timeout или возраст lock-файла не разрешает отобрать ownership у живого writer.
- Readers immutable snapshots держат read lease конкретной generation (S9-16/S9-37); parent не мутируется и не освобождается, пока на него есть ссылки.
- Реализовано (S9-04 частично, `ba77773`, [`storage_io.rs`](../../crates/od-host/src/storage_io.rs)): `flock(LOCK_EX|LOCK_NB)` на том же fd, что используется для записи; identity `(dev, ino)` проверяется после lock и перед каждой мутацией; symlink/hard-link aliases попадают на тот же lock; lock освобождается при SIGKILL процесса (тест с re-spawned бинарником). Lock advisory: процесс, не вызывающий `flock`, не остановлен. Read lease реализован (`31b8c2a`): shared `flock`, конфликт с writer в обе стороны, identity-проверка; `open_read_only` без lease не является lease. Busy-политика export/repair и `ParentLease` реализованы (`bc7b675`, [`storage_policy.rs`](../../crates/od-host/src/storage_policy.rs)). Публичный путь подключения диска VM (`attach_persistent_nvme` → `PersistentNvmeBackend::open`, [`persistent_nvme.rs`](../../crates/od-host/src/persistent_nvme.rs)) проверен между процессами (2026-11-18): второй процесс получает `WriterBusy`, после SIGKILL первого открытие проходит и видит подтверждённую запись. Parent в цепочке: uid проверяет `SnapFile::open`, identity — `ParentLease` (S9-16); resolver есть только в офлайн-CLI (`PathResolver` в [`tools/image-tool`](../../tools/image-tool/src/lib.rs)), VMHost цепочку не открывает. Не сделано: интеграция с VMHost lifecycle (VMHost пока не подключает гостю диски). Non-advisory lock не предоставляется: на поддержанной комбинации lock advisory, сетевые ФС вне гарантий (§3, план §1). Поведение на сетевых и non-APFS volumes не проверено.

## 6. Уровни подтверждения записи

| Уровень | Определение | Что гарантирует | Что не гарантирует |
|---|---|---|---|
| `submitted` | Запрос принят storage layer | Ничего, кроме приёма | Что write произошёл |
| `completed` | Host write call вернул полный успех (не short write) | Bytes переданы host OS | Сохранность после crash host/power loss |
| `visible` | Последующий read через тот же storage layer видит данные | Read-after-write в живом процессе | Durability |
| `durable_ack` | Все действия выбранного durability protocol (§7) успешно завершены, включая metadata/namespace там, где они нужны | После любого отказа из модели §8 recovery не возвращает состояние старше этого подтверждения | Сохранность при отказах вне модели §8 |
| `uncertain` | Ошибка/timeout/потеря ответа после возможного начала commit | Только одно из целых состояний, разрешённых transaction protocol | Rollback; успех; безопасность повторения |

Правила:

1. `durable_ack` выдаётся только после всех persistent dependencies. Порядок в CoW v1 ([`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), S9-08): data/map и копия корня поколения N+1 (последний сектор файла) записываются до первого barrier → barrier 1 → корень в неактивный slot → barrier 2 → ack; create записывает пустой корень поколения 1 (slot 1) и пустой корень поколения 2 (slot 0) под одним barrier до ack; первый пользовательский commit — поколение 3. Правило: после подтверждённого create пустой слот означает повреждение, а не норму. Порядок нормативен в v1 (§9.4); проверен на модели отказов (fault backend); испытания отключения питания S9-39 точки между барьерами 1 и 2 не покрывают.
2. Многосекторная guest-запись не становится atomic transaction автоматически; атомарность data write задаётся контрактом guest device (S9-13). Атомарность публикации root не заменяет data durability.
3. `uncertain` переводит writer в состояние `reconcile_required`: новые writes запрещены, image переоткрывается/инспектируется, исход определяется по generation/operation ID. Blind retry запрещён.
4. Законные последующие overwrite/discard учитываются; требование не означает вечного хранения старых версий. Referenced snapshots сохраняют свои данные.

## 7. Host durability API — статус: частично реализовано, не подтверждено испытаниями

- Реализовано (`69e0b02`, [`storage_io.rs`](../../crates/od-host/src/storage_io.rs), [`io.rs`](../../crates/od-storage/src/io.rs)): durability данных файла на macOS через `fcntl(F_FULLFSYNC)` на заблокированном writable дескрипторе. `DurableAck` создаётся в checked-слое только после успеха backend. Явный отказ (ENOTSUP) даёт `UnsupportedDurability` без смены состояния; любой другой сбой flush, включая `EINVAL`, переводит `CheckedStorage` в sticky `reconcile_required` (запись, resize и flush отклоняются, `into_backend` не выдаёт backend).
- Не реализовано: namespace durability (create, rename, directory entry) остаётся `unsupported`. На не-macOS и без writer-lock durability не объявляется. `RawImage` ([`raw.rs`](../../crates/od-storage/src/raw.rs)) сам sync не вызывает.
- Источник семантики `F_FULLFSYNC`: локальная man-страница `fcntl(2)`, macOS 26.7.1 (проверено 2026-11-05): `fsync` плюс запрос накопителю сбросить буферы; barrier для ранее `fsync`-нутых данных на том же устройстве; реализован на HFS, FAT, UDF и APFS; некоторые FireWire-накопители игнорируют запрос. Rust std (Context7, 2026-11-05) платформенный механизм `sync_all` и namespace durability не описывает.
- Не подтверждено: сопоставление ENOTSUP/EINVAL — man-страница не описывает коды ошибок `F_FULLFSYNC`, поэтому это защитное предположение. Исполнение `F_FULLFSYNC` на конкретном внутреннем SSD и поведение накопителя не проверены испытаниями уровней 1–3 ([STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md)); power-loss не проверен; namespace durability источником не подтверждена.
- Любое durable-обещание за пределами file-data на macOS отвечает явным `unsupported_durability` (§11), а не успехом.
- **Объём v1 (решение владельца 2026-11-18, S9-06 закрыта в этом объёме).** Durable-гарантия v1 — только данные файла: `DurableAck` после успешного `F_FULLFSYNC` на macOS. Порядок вызовов commit проверен на реальном файле, kill процесса — уровень 1 ([STORAGE_RECOVERY.md §4.2](../testing/STORAGE_RECOVERY.md)). Создание, переименование и публикация новых файлов (export, import, salvage, compaction, branch) — завершение без durable-гарантии для записи каталога; после сбоя хоста новое имя может отсутствовать, хотя содержимое файла было сброшено. Это предел v1 ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)), а не гарантия.
- Запись: [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed) (открыта для namespace, power-loss и non-APFS/network томов).

## 8. Модель отказов (crash model)

Модель задаёт, какие отказы обязаны переживаться без потери `durable_ack`. Методика испытаний каждого уровня — [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md).

Предусмотренные отказы:

| Отказ | Допущение модели |
|---|---|
| Завершение процесса writer в любой точке | Host caches живы; completed writes видимы после reopen; durability не проверяется |
| Crash/перезапуск host OS | Данные без `durable_ack` могут быть потеряны полностью, частично или в произвольном порядке |
| Потеря питания выбранной комбинации §3 | То же; дополнительно рассматриваются volatile caches накопителя |
| Unsynced writes | Могут быть потеряны, переупорядочены и разорваны (torn) на любой границе; atomic sector write **не** предполагается |
| Host I/O errors | ENOSPC, quota, EIO, short write, permission, interrupted call — явные результаты |

Вне модели (не обещается переживать; при возможности обнаруживается):

- Нарушение durability-обещания самим накопителем/контроллером/filesystem после успешного протокола §7 — допущение, требующее evidence S9-39, а не доказанное свойство.
- Тихая порча уже durable данных носителем — обнаруживается в checksum scope (§9), не исправляется.
- Намеренная модификация файлов другим процессом или атакующим; checksum не является authentication.
- Одновременная запись другим writer в обход §5.

Обязательные свойства recovery:

- Recovery не возвращает состояние старше последнего `durable_ack`. В CoW v1 это достигается так: create заполняет оба слота (пустой слот после ack — повреждение, ISSUE-057 закрыт `01ef35d`), writable open с необъяснимым повреждением отклоняется (`DamagedOtherSlot`), salvage показывает предыдущее поколение с `possible_lost_commit`. Повреждение вне модели §8 обнаруживается и не заявляется как покрытое.
- Interrupted unpublished candidate и corruption опубликованного durable state различаются. Нельзя молча откатиться на старую валидную копию, потеряв подтверждённые данные. При невозможности доказать нормальный исход — diagnostic/read-only open или отказ; salvage — отдельная операция в новый target (S9-21).
- Самый большой generation с валидным header ещё не гарантирует пригодный root.

## 9. CoW v1 (нормативно с 2026-11-06)

Статус: принят владельцем проекта как нормативный (решение пользователя, 2026-11-06). Нормативна часть v1: раскладка корня, карта, лимиты, протокол коммита, create и выбор корня (§9.1–9.7). Слой snapshot-метаданных (ODSNAP, бит building, правила цепочек) **в принятие не входит**: он описан в коде, но нормативных bytes не имеет (§9.8). S9-40 частичный: throughput, чтение, open/roll-forward, память, цепочки, export и compaction измерены, структуры по данным не меняются; checkpoint/restore заблокированы S9-29…31. Это пределы v1, а не гарантии. Реализация: [`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), [`cow_parse.rs`](../../crates/od-storage/src/cow_parse.rs), [`cow_tx.rs`](../../crates/od-storage/src/cow_tx.rs).

### 9.1 Раскладка файла

```text
[0 .. 512)      root slot 0   (поколения 0, 2, 4, ...; поколение g лежит в слоте g & 1)
[512 .. 1024)   root slot 1   (поколения 1, 3, 5, ...)
[1024 .. E)     append-only область; E — end_offset корня с наибольшим поколением
```

Все целые числа little endian. `E` — `end_offset` выбранного (опубликованного) корня, а не размер файла. Данные блоков пишутся только за `E` текущего корня; опубликованные байты не перезаписываются. Байты за `E` (хвост оборванного или отменённого кандидата) не принадлежат ни одному состоянию: open их игнорирует и сообщает длину (`tail_len`), следующий коммит пишет начиная с `E` поверх хвоста. Исключение — копия корня в последнем секторе, см. roll-forward в §9.6.

### 9.2 Root slot (512 Б)

| Смещение | Размер | Поле |
|---|---|---|
| 0 | 8 | magic `ODCOWP\0\x01` |
| 8 | 4 | version = 1 |
| 12 | 4 | block size = 512 (identity) |
| 16 | 8 | generation |
| 24 | 8 | block_count (логических блоков) |
| 32 | 8 | map_offset |
| 40 | 4 | CRC-32 карты |
| 44 | 8 | end_offset |
| 52 | 8 | mandatory feature bits (v1: 0) |
| 60 | 8 | optional feature bits (v1: 0) |
| 68 | 440 | reserved, должны быть нулями |
| 508 | 4 | CRC-32 байтов 0..508 |

CRC-32 — IEEE, полином `0xEDB88320` (reflected), init и xorout `0xFFFFFFFF`. Карта — плоский массив записей по 12 Б `{physical_offset: u64, block_crc: u32}`, по одной на логический блок, записывается целиком при каждом коммите. Поколение `0` и `u64::MAX` недопустимы.

### 9.3 Лимиты и ошибки v1

- `MAX_BLOCKS = 1 << 16`; `MAX_FILE_BYTES = 1 << 32`; `MAX_TX_BLOCKS = 256` на один коммит; `MAX_MAP_BYTES = MAX_BLOCKS × 12`.
- Значения из файла проверяются **до** выделения памяти и обхода карты.
- Неизвестный mandatory-бит — отказ `unsupported_mandatory_feature`. Допустимая маска mandatory-битов зависит от возможностей backend и режима доступа. В v1 оба слова нулевые; ненулевые optional-биты сейчас принимаются молча, правило для них не определено и будет задано вместе с первой версией, которая их использует.

### 9.4 Протокол коммита

Кандидат дописывается за `E`: `[новые блоки][карта][выравнивание до 512 нулями][копия корня]`. Копия корня — последний сектор файла.

1. Запись блоков, карты и копии корня.
2. Барьер 1 (`F_FULLFSYNC` на macOS).
3. Корень N+1 записывается в неактивный слот.
4. Барьер 2. `DurableAck` выдаётся только после него; состояние в памяти продвигается после ack.

Отмена возможна только до записи копии корня. Ошибка ввода-вывода после начала барьера или записи корня делает сессию poisoned: нужно переоткрыть. Ошибки проверки вызова (например, превышение `MAX_TX_BLOCKS`) отклоняются до обращения к backend и сессию не отравляют.

### 9.5 Create

`create` пишет пустой корень поколения 1 в слот 1 и пустой корень поколения 2 в слот 0 под одним барьером. Первый пользовательский коммит — поколение 3. После подтверждённого create пустой слот — повреждение, а не норма. Файл, в котором ровно один валидный пустой корень при размере заголовка, — незавершённый create (`IncompleteCreate`): writable open отклоняет, read-only сообщает `NoValidRoot`. [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) закрыт (`01ef35d`).

### 9.6 Классы слота и выбор корня

- **Пустой**: все байты нулевые.
- **Повреждённый**: неверный magic или CRC — признак оборванной или испорченной записи.
- **CRC-валидный, но недопустимый** (неизвестная версия, геометрия, mandatory-бит, поколение `0` или `u64::MAX`, поля вне файла): отказ, а не повреждение. Читатель не откатывается на другой слот.
- **Валидный**.

Выбор: при одном валидном корне берётся он. При двух валидных поколения должны быть `N` и `N−1`; иначе открытие отказывает (`GenerationConflict`, `RootInconsistent`).

**Roll-forward (добавлено 2026-11-18 по итоговому ревью S9-43; описывает поведение реализации v1, [`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs) `select_root`/`find_root_copy`/`recover`).** Если второй слот не валиден (пустой или повреждённый), а выбран корень поколения `N`, читатель проверяет последний 512-байтный сектор файла. Он принимается как кандидат `N+1`, только если одновременно:
- сектор лежит не раньше `E` выбранного корня;
- он разбирается как CRC-валидный допустимый корень для слота поколения `N+1`;
- его поколение ровно `N+1`;
- его `end_offset` равен размеру файла.

Неразборчивая или недопустимая копия считается отсутствующей; ошибка ввода-вывода при чтении последнего сектора — ошибка открытия. Карта кандидата и каждый блок, на который она ссылается, проверяются по CRC целиком. Любое расхождение — отказ открытия, а не откат на корень `N`: копия была сброшена до барьера 1, поэтому неполный кандидат означает повреждение. Writable open записывает принятую копию в её слот и выполняет барьер до первого использования (`rolled_forward`). Read-only open показывает состояние `N+1`, ничего не записывая.

**Необъяснённое повреждение.** Если второй слот не валиден, а допустимой копии нет, выбирается корень `N` с признаком `possible_lost_commit`: подтверждённый коммит `N+1` мог быть потерян. Read-only open и salvage показывают `N` с этим признаком; writable open отказывает (`DamagedOtherSlot`).

Salvage ([`cow_salvage.rs`](../../crates/od-storage/src/cow_salvage.rs)) читает слой через `load_state`, не откатывается на старый корень при нечитаемой карте и не копирует блоки без подтверждённой provenance. Неупорядоченные корни дают `Partial` (причина `roots_disagree` или `unordered_roots`); ни одна версия при этом не выдаётся как `Complete`.

### 9.7 Совместимость и миграция (S9-22)

| Версия | Чтение | Запись | Миграция |
|---|---|---|---|
| v1 | да | да | — (текущая) |
| v2 и новее | нет (неизвестная версия → отказ) | нет | не определена: вторая версия не существует |
| RAW (вход) | как источник `import` | нет | `od-image-tool import` в новый base image v1 (S9-20): единственный поддержанный путь обновления; источник не меняется |

Неизвестные версии, неизвестные mandatory-биты и неверная геометрия — типизированный отказ, не частичное чтение. Downgrade не поддерживается. Миграция проектируется только при появлении второй версии; она обязана сначала писать в новый target и проверять logical data, snapshots/parent links и identities. Frozen fixture [`v1_created_committed.img`](../../crates/od-storage/tests/fixtures/cow/v1_created_committed.img) (2560 Б) служит регрессией чтения v1.

### 9.8 Не нормативно в v1: слой snapshot

Реализация обязана соблюдать следующие инварианты, но их bytes в v1 не определены и принятием не закрываются: опубликованные roots и snapshot parents неизменяемы; reachable data не перезаписывается и не освобождается без доказанного отсутствия references и leases; sparse inheritance, explicit zero и discard различимы; удаление child mapping не раскрывает старые parent bytes; repair, migration и export создают новый target. Формат ODSNAP (header, kind, бит building, `MAX_SNAPSHOTS`, `MAX_DISK_BLOCKS`, записи) будет описан отдельно.

Раздел v1 определяет раскладку корня, карту, поколения, CRC-32, два root slot, барьеры и правила create/recovery. Journal, B-tree и refcounts в v1 не добавлены.

## 10. Checksum scope

- Checksum обнаруживает случайное повреждение в заявленных пределах выбранного алгоритма; он не является authentication, confidentiality или доказательством целостности всего reachable graph.
- Scope v1 (нормативно, §9.2): CRC-32 корня, CRC-32 карты и `block_crc` каждого блока данных в записи карты. Snapshot-метаданные ODSNAP — по реализации, не нормативно (§9.8).
- Алгоритм v1 — CRC-32 IEEE (§9.2), собственная реализация `crc32` в `cow_proto.rs`.
- Секреты vTPM и чувствительное состояние требуют отдельной защиты (S9-27); checksum её не даёт.

## 11. Явные результаты ошибок

Storage API возвращает типизированные результаты: `out_of_range`, `short_read`/`short_write`, `io_error`, `no_space`, `quota_exceeded`, `permission_denied`, `busy` (writer/lease), `unsupported_feature`, `unsupported_durability`, `corrupted_metadata` (с местом и типом), `uncertain_commit`. Ни один из них не превращается в successful zero read, чистый shutdown или гостевой CPU fault. Точные имена типов задаёт S9-03.

## 12. Открытые вопросы и blockers

| Вопрос | Блокирует | Следующий шаг |
|---|---|---|
| Namespace durability на APFS; подтверждение `F_FULLFSYNC` испытаниями и источником | Предел v1 ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); S9-06 закрыта в суженном объёме, `durable_ack` для создания и публикации не даётся | Источник и испытания уровней 2–3 (уровень 1 выполнен); подтверждение `F_FULLFSYNC` на носителе — S9-39 |
| VMHost lifecycle (две VMHost на одном backing) | S9-04 (приёмка) | Заблокировано: VMHost не подключает гостю диски; `flock`, alias identity, read lease и путь `PersistentNvmeBackend::open` покрыты multiprocess tests (`ba77773`, `31b8c2a`, 2026-11-18). Resolver цепочки есть только в офлайн-CLI (`PathResolver` с `ParentLease`); non-advisory lock на сетевых ФС не поддерживается (вне scope) |
| LBA size guest device | S9-13 | NVMe-контроллер S7-16 ([NVME_DEVICE.md §13.3](NVME_DEVICE.md#133-backend-и-namespace)) — 512 B; подтвердить при подключении образа этапа 9 гостю (S9-13) |
| CoW block size, map layout, checksum алгоритм | Решено: формат v1 принят как нормативный владельцем 2026-11-06 (§9). Остаётся открытым только влияние на производительность | Измерено (S9-40): commit-latency ≈4 мс на барьер ([S9_40_COMMIT_LATENCY.md](../testing/S9_40_COMMIT_LATENCY.md)), throughput/WA/чтение/open/память против RAW ([S9_40_THROUGHPUT.md](../testing/S9_40_THROUGHPUT.md)), цепочки, export и compaction ([S9_40_CHAIN_EXPORT_COMPACT.md](../testing/S9_40_CHAIN_EXPORT_COMPACT.md)); решение по данным — структуры v1 не меняются, WA commit по расчёту линейна по размеру карты (известный предел), open цепочки определяется объёмом проверяемых данных, а не глубиной; checkpoint/restore не измерены (S9-29…31) |
| Модель накопителя/контроллера внутреннего SSD | S9-39 claim | Зафиксировать при подготовке стенда |


## 13. Контейнер VM v1 (S9-23)

Статус: **реализован локальный кодек, формат не принят как нормативный** (нет ревью и нет настоящих участников). Код: [`vm_container.rs`](../../crates/od-storage/src/vm_container.rs), тесты [`vm_container_tests.rs`](../../crates/od-storage/src/vm_container_tests.rs); модуль собирается при `cfg(any(test, feature = "test-support"))`, как CoW-модули. Контейнер — чисто байтовый кодек: он не создаёт данные участников и не знает семантики секций. CPU, RAM, устройства, UEFI и vTPM приходят от вызывающего как непрозрачные байты (inline) или как ссылки на отдельные файлы (reference). Состав обязательных секций задаёт вызывающий по профилю ([VM_STATE.md §5](VM_STATE.md#5-участники-контейнера-по-профилям-s9-23)).

### 13.1 Раскладка (little endian)

Файл: header (128 Б) → таблица секций (`count × 64` Б) → payload всех секций подряд, без дыр и без хвоста. Писатель пишет записи таблицы и payload в каноническом порядке `(kind, instance)`, поэтому один и тот же набор секций даёт одни и те же байты при любом порядке входа. Читатель принимает записи таблицы в любом порядке и payload в любом порядке без дыр и пересечений (offset каждой записи берётся из таблицы); результат чтения всегда отсортирован по `(kind, instance)`.

| Header, байты | Поле |
|---|---|
| 0..8 | magic `ODVMCK\0\x01` |
| 8..12 | version u32 = 1 |
| 12..16 | header_len u32 = 128 |
| 16..24 | mandatory feature bits u64: в v1 обязаны быть 0 |
| 24..32 | optional feature bits u64: в v1 игнорируются |
| 32..48 | vm_identity (16 Б) |
| 48..56 | checkpoint_id u64 |
| 56..64 | epoch u64, не 0 и не `u64::MAX` |
| 64..68 | profile u32 |
| 68..72 | section_count u32 |
| 72..80 | total_len u64 (длина всего файла) |
| 80..84 | CRC-32 таблицы секций |
| 84..124 | reserved, нули |
| 124..128 | CRC-32 байт 0..124 |

| Запись таблицы, байты | Поле |
|---|---|
| 0..4 | kind u32 |
| 4..8 | flags u32: бит 0 mandatory, бит 1 reference (иначе inline); остальные биты — reserved, нули |
| 8..12 | instance u32 |
| 12..16 | schema_id u32 |
| 16..20 | schema_version u32 |
| 20..28 | offset u64 (от начала файла) |
| 28..36 | len u64 (не 0) |
| 36..44 | epoch u64, равен epoch header |
| 44..48 | CRC-32 payload |
| 48..64 | reserved, нули |

Reference payload — ровно 40 Б: file_uid (16 Б), generation u64, byte_len u64 (не 0), CRC-32 байтов, на которые указывает ссылка, u32, 4 Б нулей. Путей, host handles и указателей нет; file_uid из одних нулей отвергается. Кодек не открывает файлы по ссылке и их содержимое не проверяет — это делает участник при restore.

### 13.2 Лимиты

`MAX_SECTIONS = 256`, `MAX_SECTION_BYTES = 1 << 26` (64 MiB), `MAX_TOTAL_BYTES = 1 << 28` (256 MiB). Значения из файла (section_count, total_len, len записи) сверяются с лимитами до использования таблицы и payload; читатель заимствует payload из входного буфера и выделяет только структуры размером не более `MAX_SECTIONS`; писатель проверяет лимиты до заполнения выходного буфера. Числа — стартовые параметры, не результат замеров (см. §13.5).

### 13.3 Правила и ошибки

Порядок проверок читателя: magic → version → header_len → CRC header → reserved и feature bits header → epoch → лимиты header → identity/checkpoint_id/profile → `total_len` равен длине файла → граница таблицы → CRC таблицы → каждая запись (reserved и флаги, `MAX_SECTION_BYTES`, len ≠ 0, `offset + len` без переполнения и в пределах файла, epoch) → дубликаты → отсутствие пересечений и дыр → CRC payload и форма reference → unknown kinds → обязательные секции.

| Ошибка | Условие |
|---|---|
| `BadMagic`, `BadVersion`, `BadHeaderLen` | Неверные magic, version или header_len |
| `HeaderCrc`, `TableCrc`, `SectionCrc` | Несовпадение CRC-32 header, таблицы, payload секции |
| `ReservedNonZero`, `UnsupportedHeaderFlags` | Ненулевые reserved-байты, неизвестные биты flags записи, ненулевые mandatory bits header |
| `BadEpoch`, `EpochMismatch` | Epoch header равен 0 или `u64::MAX`; epoch секции ≠ epoch header |
| `IdentityMismatch` | vm_identity, checkpoint_id или profile не совпали с ожидаемыми |
| `MissingRequiredSection` | Нет обязательной (kind, instance); проверяется и при записи, и при чтении |
| `RequiredNotMandatory` | Только при записи: обязательная по профилю (kind, instance) передана с mandatory = false |
| `DuplicateSection` | Повтор (kind, instance) |
| `UnknownMandatory` | Секция с неизвестным вызывающему kind и флагом mandatory. Неизвестная optional секция пропускается (перечисляется в `skipped_optional`), но проходит все проверки структуры и CRC |
| `Overlap`, `Gap`, `OutOfBounds`, `Overflow` | Пересечение с другой секцией или с таблицей; байты, которые не покрыты ни одной секцией; выход за `total_len`, длина файла ≠ `total_len`; переполнение `offset + len` |
| `LimitExceeded` | Превышены `MAX_SECTIONS`, `MAX_SECTION_BYTES`, `MAX_TOTAL_BYTES` |
| `EmptySection`, `BadReferenceLen`, `BadReference` | Payload нулевой длины; reference не 40 Б; reference с нулевым file_uid или byte_len |

Писатель не подставляет нули и не создаёт placeholder: отсутствующий обязательный участник, обязательный участник с mandatory = false (иначе читатель, не знающий kind, пропустил бы его), пустой inline payload и нулевая ссылка — ошибки записи. Читатель требует обязательные секции и не пропускает неизвестное mandatory-состояние. Ни одна ошибка не превращается в успешное чтение с заменой данных.

### 13.4 Что контейнер не гарантирует

CRC-32 обнаруживает случайные повреждения; он не аутентифицирует контейнер, не защищает от намеренной подмены (подмену с пересчётом CRC обнаружить нельзя) и не доказывает целостность файлов, на которые указывают reference-секции. Секреты vTPM и защита ключей — S9-27 (§10). Атомарная публикация (все artifacts durable → manifest durable → ack, [VM_STATE.md §3.3](VM_STATE.md#33-таблица-a--prerequisites-consistency-durability)), права доступа к файлу и привязка к host — вне кодека. Совпадение epoch между контейнером и самими reference-файлами проверяет участник по `generation`.

### 13.5 Открытые вопросы S9-23

| Вопрос | Решение в коде / рекомендация |
|---|---|
| CRC-32 таблицы секций | Утверждённый дизайн задавал CRC только header и payload; поля записи (kind, flags, instance, schema, epoch) тогда не защищены. Добавлено поле «CRC-32 таблицы» в header (байты 80..84, ранее резерв). Рекомендация: подтвердить при ревью |
| Лимиты 256 / 64 MiB / 256 MiB | Выбраны без замеров. Рекомендация: уточнить после S9-24/25, когда известен реальный размер inline-секций CPU/устройств |
| Нулевая длина payload и нулевой file_uid | Отвергаются как возможный placeholder. Рекомендация: подтвердить; если участнику нужно «пусто», он должен выражать это своей схемой в непустом payload |
| Режим секции (inline или reference) для данного kind | Кодек возвращает режим, но не требует конкретный режим от вызывающего; проверка режима для required kind — на стороне участника. Рекомендация: добавить в `Expect` при появлении первого участника |
| Число instance для required-kind (например число vCPU) | Кодек проверяет только присутствие перечисленных в `required` пар (kind, instance); полноту набора instance он не знает. Участник обязан сверить число instance с фактическим (например число vCPU по конфигурации VM) при записи и при restore: лишний или недостающий instance, не попавший в `required`, кодек не обнаружит. Рекомендация: подтвердить при ревью |
| Какие kind и schema у участников | Не задаются кодеком; назначаются вместе с S9-24…27 |
| Привязка к операции (operation ID, [VM_STATE.md §3.2](VM_STATE.md#32-operation-id)) и формат operation record | Не входят в контейнер; формат record — S9-33 |
| Нормативный статус | Требуется архитектурное ревью и принятие владельцем; до этого раздел не нормативен |
