# Контракт persistent форматов хранения

Статус: **S9-00 принято с оговорками по архитектурному ревью; формат CoW не принят**. Документ фиксирует scope, границы, модель отказов, лимиты и правила подтверждения. Нормативных байтов здесь пока нет. Реализованы (коммиты `a59338a`, `ba77773`, `4fcd5c4`, `69e0b02`): контракт [`io.rs`](../../crates/od-storage/src/io.rs) (`StorageBackend`, `CheckedStorage`), [`raw.rs`](../../crates/od-storage/src/raw.rs) (`RawImage`), host backend с `flock` и flush данных файла в [`storage_io.rs`](../../crates/od-host/src/storage_io.rs). Есть прототип CoW (S9-08…12; коммиты `000c1da`, `17c9ce6`, `61a99e0`, `7d23bf9`): [`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), [`cow_parse.rs`](../../crates/od-storage/src/cow_parse.rs), [`cow_tx.rs`](../../crates/od-storage/src/cow_tx.rs) с тестами на fault backend ([`testing/fault.rs`](../../crates/od-storage/src/testing/fault.rs), `a7874b8`); он не нормативен и не принят: размер блока, раскладка корня и карта — гипотезы до замеров S9-40, открыт [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable). Snapshot metadata и full-VM container — в S9-14 и S9-23 (не начаты). Namespace durability не реализована, power-loss не проверен.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [этап 9 roadmap](../ROADMAP.md#этап-9-надёжность-данных-и-управление-состоянием), [архитектура §7](../ARCHITECTURE.md#7-диски-конфигурация-и-снимки), [виртуальная плата](VIRTUAL_BOARD.md), [матрица поддержки](SUPPORT_MATRIX.md), [состояние VM и операции](VM_STATE.md), [методика recovery](../testing/STORAGE_RECOVERY.md).

## 1. Область документа

Документ описывает долговечные bytes, которые Open Desktop пишет на host storage и потом читает обратно:

| Artifact | Владелец кода | Статус формата |
|---|---|---|
| RAW disk image | `crates/od-storage/src/` | Без header: ровно `logical_size` bytes guest LBA space. Семантика в §4–§7; реализован `RawImage` ([`raw.rs`](../../crates/od-storage/src/raw.rs), `4fcd5c4`), ревью S9-05 пройдено, power-loss не проверен |
| Собственный CoW disk image | `crates/od-storage/src/` | **Прототип, не нормативен, не принят (S9-08…12, S9-40)**: [`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), [`cow_parse.rs`](../../crates/od-storage/src/cow_parse.rs), [`cow_tx.rs`](../../crates/od-storage/src/cow_tx.rs); требования и статус — §9; нормативные bytes не определены |
| Snapshot metadata и цепочки | `crates/od-storage/src/` | **TBD S9-14** поверх S9-08 |
| Full-VM checkpoint container и manifest | container — `od-storage`, состав участников — `od-machine` | **TBD S9-23**; список участников — в [VM_STATE.md](VM_STATE.md) |
| UEFI variable store artifact | семантика — `firmware/uefi/src/`, persistence — `od-storage` | **TBD S9-26**; firmware отсутствует |
| vTPM persistent state | state — `od-machine`, persistence — `od-storage`, защита ключей — `od-host` | **TBD S9-27**; vTPM отсутствует |

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
| `offline-cow-v1` | Собственный CoW, recovery, immutable snapshots, check/export/salvage/migration | Нет | S9-08…12 реализованы на прототипном формате (не принят, S9-40, ISSUE-057); S9-14…22 не начаты. S9-13 (guest storage/flush) относится к VM-профилям |
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

Все значения ниже — **параметры первого профиля**. Значения на горячем пути (размер CoW block, структура map) — гипотезы до прототипных замеров S9-08 по правилам `AGENTS.md`; изменение значения не меняет правила проверки.

| Параметр | Правило | Начальное значение | Статус |
|---|---|---|---|
| Guest logical block (LBA) size | Фиксируется при create, не меняется; все guest-visible смещения кратны ему | 512 B кандидат; 4096 B — альтернатива для S9-13 по требованиям NVMe/Windows | Гипотеза до S9-13 |
| Logical size RAW | Кратен LBA size; `> 0`; checked arithmetic до любого I/O | В [`raw.rs`](../../crates/od-storage/src/raw.rs) проверяются `size > 0`, block size — степень двойки и кратность; явного верхнего лимита профиля нет, действует только `max_len` backend (`StorageCapabilities`). S9-05 лимит не задал | Открытый вопрос; владелец решения — S9-00 (границы профиля), применение в `RawImage` — S9-05 |
| CoW block size | Степень двойки, кратна LBA size, фиксирована на image | Выбирается замерами S9-08 | Гипотеза |
| Virtual capacity CoW | Явный maximum в header; map bounded | S9-08 | TBD |
| Глубина snapshot chain | Явный maximum; превышение — отказ open/create, не обход | S9-14 | TBD |
| Число snapshots/roots | Явный maximum | S9-14 | TBD |
| Размер metadata structures | Каждая длина проверяется против лимита до allocation/read | S9-08/S9-09 | TBD |

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
- Реализовано (S9-04 частично, `ba77773`, [`storage_io.rs`](../../crates/od-host/src/storage_io.rs)): `flock(LOCK_EX|LOCK_NB)` на том же fd, что используется для записи; identity `(dev, ino)` проверяется после lock и перед каждой мутацией; symlink/hard-link aliases попадают на тот же lock; lock освобождается при SIGKILL процесса (тест с re-spawned бинарником). Lock advisory: процесс, не вызывающий `flock`, не остановлен. Read lease реализован (`31b8c2a`): shared `flock`, конфликт с writer в обе стороны, identity-проверка; `open_read_only` без lease не является lease. Не сделаны busy-политика для export/repair, запрет изменения parent, интеграция с VMHost. Поведение на сетевых и non-APFS volumes не проверено.

## 6. Уровни подтверждения записи

| Уровень | Определение | Что гарантирует | Что не гарантирует |
|---|---|---|---|
| `submitted` | Запрос принят storage layer | Ничего, кроме приёма | Что write произошёл |
| `completed` | Host write call вернул полный успех (не short write) | Bytes переданы host OS | Сохранность после crash host/power loss |
| `visible` | Последующий read через тот же storage layer видит данные | Read-after-write в живом процессе | Durability |
| `durable_ack` | Все действия выбранного durability protocol (§7) успешно завершены, включая metadata/namespace там, где они нужны | После любого отказа из модели §8 recovery не возвращает состояние старше этого подтверждения | Сохранность при отказах вне модели §8 |
| `uncertain` | Ошибка/timeout/потеря ответа после возможного начала commit | Только одно из целых состояний, разрешённых transaction protocol | Rollback; успех; безопасность повторения |

Правила:

1. `durable_ack` выдаётся только после всех persistent dependencies. Порядок в прототипе CoW ([`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), S9-08): data/map и копия корня поколения N+1 (последний сектор файла) записываются до первого barrier → barrier 1 → корень в неактивный slot → barrier 2 → ack; create записывает поколение 1 с одним barrier до ack. Порядок проверен только на модели отказов (fault backend) и не принят как нормативный (S9-40, ISSUE-057).
2. Многосекторная guest-запись не становится atomic transaction автоматически; атомарность data write задаётся контрактом guest device (S9-13). Атомарность публикации root не заменяет data durability.
3. `uncertain` переводит writer в состояние `reconcile_required`: новые writes запрещены, image переоткрывается/инспектируется, исход определяется по generation/operation ID. Blind retry запрещён.
4. Законные последующие overwrite/discard учитываются; требование не означает вечного хранения старых версий. Referenced snapshots сохраняют свои данные.

## 7. Host durability API — статус: частично реализовано, не подтверждено испытаниями

- Реализовано (`69e0b02`, [`storage_io.rs`](../../crates/od-host/src/storage_io.rs), [`io.rs`](../../crates/od-storage/src/io.rs)): durability данных файла на macOS через `fcntl(F_FULLFSYNC)` на заблокированном writable дескрипторе. `DurableAck` создаётся в checked-слое только после успеха backend. Явный отказ (ENOTSUP) даёт `UnsupportedDurability` без смены состояния; любой другой сбой flush, включая `EINVAL`, переводит `CheckedStorage` в sticky `reconcile_required` (запись, resize и flush отклоняются, `into_backend` не выдаёт backend).
- Не реализовано: namespace durability (create, rename, directory entry) остаётся `unsupported`. На не-macOS и без writer-lock durability не объявляется. `RawImage` ([`raw.rs`](../../crates/od-storage/src/raw.rs)) сам sync не вызывает.
- Источник семантики `F_FULLFSYNC`: локальная man-страница `fcntl(2)`, macOS 26.7.1 (проверено 2026-11-05): `fsync` плюс запрос накопителю сбросить буферы; barrier для ранее `fsync`-нутых данных на том же устройстве; реализован на HFS, FAT, UDF и APFS; некоторые FireWire-накопители игнорируют запрос. Rust std (Context7, 2026-11-05) платформенный механизм `sync_all` и namespace durability не описывает.
- Не подтверждено: сопоставление ENOTSUP/EINVAL — man-страница не описывает коды ошибок `F_FULLFSYNC`, поэтому это защитное предположение. Исполнение `F_FULLFSYNC` на конкретном внутреннем SSD и поведение накопителя не проверены испытаниями уровней 1–3 ([STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md)); power-loss не проверен; namespace durability источником не подтверждена.
- Любое durable-обещание за пределами file-data на macOS отвечает явным `unsupported_durability` (§11), а не успехом.
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

- Recovery не возвращает состояние старше последнего `durable_ack`. Оговорка: в прототипе CoW гарантия не действует при двойном повреждении (обнулённый slot поколения 2 вместе с повреждённой копией корня неотличим от обрыва первого commit; writable open откроет поколение 1) — [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable); повреждение вне модели §8 и должно обнаруживаться, а не заявляться как покрытое.
- Interrupted unpublished candidate и corruption опубликованного durable state различаются. Нельзя молча откатиться на старую валидную копию, потеряв подтверждённые данные. При невозможности доказать нормальный исход — diagnostic/read-only open или отказ; salvage — отдельная операция в новый target (S9-21).
- Самый большой generation с валидным header ещё не гарантирует пригодный root.

## 9. Требования к CoW формату (bytes — TBD; прототип S9-08…12 не нормативен)

Статус: прототип протокола — два root slot с чётностью generation и копия корня N+1 до barrier 1 ([`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), строгий parser [`cow_parse.rs`](../../crates/od-storage/src/cow_parse.rs), транзакции [`cow_tx.rs`](../../crates/od-storage/src/cow_tx.rs)); **не нормативен и не принят**. Размер блока, раскладка корня, плоская карта и лимиты — гипотезы до замеров S9-40; открыт [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) (различение первого commit и подтверждённого поколения 2).

S9-08 должен определить нормативно: superblock/root descriptors, block map, generation/transaction IDs, checksums, bounded snapshot metadata, root selection, required fences, обработку unreachable garbage, несовместимые features. Кандидат плана: один файл и один writer, фиксированные virtual capacity и block size, append-only immutable extents, полная bounded map поколения, два root slots. Journal, B-tree и refcounts не добавлять без доказанной необходимости.

Инварианты, которые формат обязан обеспечить:

- Published roots и snapshot parents неизменяемы; reachable data/metadata не перезаписываются и не освобождаются до доказанного отсутствия references, readers и save/restore leases.
- Sparse inheritance, explicit zero и discard различимы; удаление child mapping не раскрывает старые parent bytes.
- Repair/migration/export создают новый target; источник не меняется по умолчанию; потерянные ranges перечисляются, а не заполняются нулями как «восстановленные».

## 10. Checksum scope

- Checksum обнаруживает случайное повреждение в заявленных пределах выбранного алгоритма; он не является authentication, confidentiality или доказательством целостности всего reachable graph.
- Scope первой версии (кандидат, уточняется S9-08): header/superblock, root descriptors, каждая map/metadata structure. Покрытие data extents — решение S9-08 с замерами; если data не покрыта, inspect/check сообщают это явно.
- Алгоритм выбирается в S9-08 по политике зависимостей; general-purpose checksum library допустима.
- Секреты vTPM и чувствительное состояние требуют отдельной защиты (S9-27); checksum её не даёт.

## 11. Явные результаты ошибок

Storage API возвращает типизированные результаты: `out_of_range`, `short_read`/`short_write`, `io_error`, `no_space`, `quota_exceeded`, `permission_denied`, `busy` (writer/lease), `unsupported_feature`, `unsupported_durability`, `corrupted_metadata` (с местом и типом), `uncertain_commit`. Ни один из них не превращается в successful zero read, чистый shutdown или гостевой CPU fault. Точные имена типов задаёт S9-03.

## 12. Открытые вопросы и blockers

| Вопрос | Блокирует | Следующий шаг |
|---|---|---|
| Namespace durability на APFS; подтверждение `F_FULLFSYNC` испытаниями и источником | S9-06 (приёмка) и `durable_ack` для создания/публикации | Источник + испытания уровней 1–3; иначе гарантия остаётся открытой |
| VMHost lifecycle, запрет изменения parent, busy-политика export/repair | S9-04 (приёмка) | Реализовать; `flock`, alias identity и read lease уже покрыты multiprocess tests (`ba77773`, `31b8c2a`) |
| LBA size guest device | S9-13 | Решить по требованиям выбранного NVMe профиля |
| CoW block size, map layout, checksum алгоритм | Принятие формата S9-08 | Прототип есть (CRC-32, плоская карта, 512-байтовый slot — гипотезы); замеры на одинаковой нагрузке (S9-40) |
| [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable): различение первого commit и подтверждённого поколения 2 | Принятие формата S9-08; гарантия «не старше `durable_ack`» при двойном повреждении | Marker-запись или счётчик в формате (S9-08/S9-40) |
| Модель накопителя/контроллера внутреннего SSD | S9-39 claim | Зафиксировать при подготовке стенда |
