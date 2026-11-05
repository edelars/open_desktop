# Этап 9. Отчёт: надёжность данных и управление состоянием

Статус отчёта: **промежуточный. Этап 9 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и тестами ветки `feat/s9-stage` на коммите `69e0b02`. Gates G1–G6 не закрыты.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md), [VM_STATE.md](../specs/VM_STATE.md), [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md), [реестр проблем](../KNOWN_ISSUES.md), [SUPPORT_MATRIX.md](../specs/SUPPORT_MATRIX.md).

## 1. Сводка

Всего задач S9-00…S9-44: 45.

| Статус | Количество | Задачи |
|---|---|---|
| Реализовано, ревью пройдено (по сведениям из задания на отчёт) | 2 | S9-03, S9-05 |
| Частично | 5 | S9-04, S9-06 (код, с оговорками ниже); S9-00, S9-01, S9-02 (только черновики документов, архитектурное ревью не выполнено) |
| `not_started` | 2 | S9-07, S9-08 |
| `blocked` | 36 | S9-09…S9-44 |

Принятого этапа или принятого gate нет. «Реализовано, ревью пройдено» относится к самой задаче и не означает принятие gate G1. Чекбоксы `[ ]` в плане не отмечены.

## 2. Задачи S9-00…S9-06

### S9-00. Область поддержки и failure model — частично (черновик)

- Коммит: `d773cc2`.
- Файлы: [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md), [VM_STATE.md](../specs/VM_STATE.md), [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md), [KNOWN_ISSUES.md](../KNOWN_ISSUES.md).
- Проверки: только документальные; кода в коммите нет.
- НЕ доказано: архитектурное ревью не выполнено. Не разрешены лимиты logical size, virtual capacity CoW, глубины цепочек, числа snapshots, LBA/CoW block size ([STORAGE_FORMAT §4](../specs/STORAGE_FORMAT.md#4-единицы-размеры-и-лимиты)). Первая host/filesystem/media комбинация (macOS 26.7.1, APFS, внутренний SSD) только предложена и не испытана.
- Расхождение, не исправлявшееся в этой работе: STORAGE_FORMAT.md (шапка: «Реализации нет», §5 и §7) и ISSUE-055 (поле «кода file I/O пока нет») описывают состояние до S9-03…06 и не отражают выбор `F_FULLFSYNC` из `69e0b02`. Требуется отдельное обновление этих документов.

### S9-01. Prerequisites по фактическому коду — частично (черновик)

- Коммит: `d773cc2`.
- Файл: [VM_STATE.md §2](../specs/VM_STATE.md#2-инвентаризация-prerequisites-s9-01).
- Проверки: инвентаризация по коду на 2026-11-05; ревью не выполнено.
- НЕ доказано: полнота инвентаризации не проверена независимо. Full-VM prerequisites заблокированы ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)).

### S9-02. Операции, состояния, результаты — частично (черновик)

- Коммит: `d773cc2`.
- Файл: [VM_STATE.md §3](../specs/VM_STATE.md#3-операции-и-состояния-s9-02); методика [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md).
- Проверки: документальные; ревью не выполнено. Ни одна описанная операция не реализована.
- НЕ доказано: согласованность контракта с реальными реализациями; открытые вопросы — [VM_STATE.md §4](../specs/VM_STATE.md#4-открытые-вопросы).

### S9-03. Узкий storage I/O interface — реализовано, ревью пройдено

- Коммит: `a59338a`.
- Файлы: [io.rs](../../crates/od-storage/src/io.rs), [io_tests.rs](../../crates/od-storage/src/io_tests.rs), [testing/contract.rs](../../crates/od-storage/src/testing/contract.rs), [testing/memory.rs](../../crates/od-storage/src/testing/memory.rs), [storage_io.rs](../../crates/od-host/src/storage_io.rs).
- Факт: `StorageBackend` и `CheckedStorage` (checked offsets, ограниченный retry short transfer и interrupt, типизированные ошибки), `DurableAck` только при объявленной durability и успешном flush. Общий contract suite исполняется на memory-backend и host file backend (Unix).
- Проверки: `memory_backend_passes_contract*`, `host_file_backend_passes_contract`, тесты overflow, short/zero progress, interrupts, inconsistent capabilities, `rejected_calls_do_not_reach_the_backend`.
- НЕ доказано: поведение на не-Unix платформах (host backend Unix-only); non-APFS и сетевые volumes.

### S9-04. Единственный writer — частично

- Коммит: `ba77773`.
- Файлы: [storage_io.rs](../../crates/od-host/src/storage_io.rs), [io.rs](../../crates/od-storage/src/io.rs).
- Сделано: `open_read_write_exclusive` и `create_new_exclusive` берут `flock(LOCK_EX|LOCK_NB)` на том же fd, что используется для записи; identity `(dev, ino)` проверяется после lock и перед каждой мутацией (`WriterIdentityLost`).
- Проверки: `second_exclusive_open_is_busy_until_first_is_dropped`, `symlink_and_hard_link_aliases_hit_the_same_lock`, `writer_in_another_process_blocks_us_and_crash_releases_it` (в т.ч. SIGKILL, перезапуск тестового бинарника), `writer_in_this_process_blocks_another_process`, `replaced_path_refuses_writes_instead_of_touching_either_file`, `unlinked_path_refuses_writes`, `read_only_open_neither_blocks_nor_is_blocked_by_writer`.
- НЕ сделано: read leases; интеграция с lifecycle VMHost; запрет изменения parent; busy-состояние диска при export/repair.
- НЕ доказано: lock advisory (known limit из коммита `4fcd5c4`): процесс, не вызывающий `flock`, может изменить файл; поведение на сетевых и non-APFS volumes. Критерий «Готово» плана («две VMHost не могут писать один backing») для реальных VMHost не проверен.

### S9-05. RAW backend — реализовано, ревью пройдено

- Коммит: `4fcd5c4`.
- Файлы: [raw.rs](../../crates/od-storage/src/raw.rs), [raw_file_tests.rs](../../crates/od-host/src/raw_file_tests.rs).
- Факт: `RawImage` с фиксированными logical size и block size; `create` не усекает и не перезаписывает существующий файл; read/write проверяются на границы до обращения к backend; короткий файл даёт `IncompleteImage`/`Truncated`, а не нули.
- Проверки: 13 unit-тестов `raw::tests` и 9 `raw_file_tests` (create exact size, refuse existing, reopen с независимым сравнением байтов, truncated-behind-open, range checks, read-only reopen, busy second writer).
- НЕ доказано: power-loss durability (flush на уровне образа см. S9-06); known limit — shrink файла процессом без `flock` между проверкой размера и записью.

### S9-06. Durability от storage до host — частично

- Коммит: `69e0b02`.
- Файлы: [storage_io.rs](../../crates/od-host/src/storage_io.rs), [io.rs](../../crates/od-storage/src/io.rs), [io_tests.rs](../../crates/od-storage/src/io_tests.rs), [raw_file_tests.rs](../../crates/od-host/src/raw_file_tests.rs).
- Сделано: **только durability данных файла** на macOS через `fcntl(F_FULLFSYNC)` на заблокированном writable дескрипторе; `DurableAck` создаётся в checked-слое только после успеха backend. Явный отказ (ENOTSUP) даёт `UnsupportedDurability` без смены состояния; любой иной сбой flush, включая `EINVAL`, переводит `CheckedStorage` в sticky reconcile-required (запись, resize, flush отклоняются без обращения к backend; `into_backend` отказывает).
- Проверки: `failed_flush_makes_storage_reconcile_required_and_sticky`, `explicit_durability_refusal_is_not_an_ack_and_not_sticky`, `contradicting_flush_is_also_reconcile_required`, `claimed_durability_must_be_backed_by_the_backend`, `sticky_storage_does_not_release_the_backend`, `flush_file_durable_after_write_on_exclusive_writer`, `flush_file_durable_refuses_lost_writer_identity`, `host_backend_declares_file_durability_only_on_macos_writer_and_lock_only_for_writer`.
- НЕ сделано / НЕ доказано:
  - Namespace durability (create, rename, directory entry) не реализована и остаётся unsupported ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); тесты `failed_namespace_flush_is_sticky` проверяют только поведение checked-слоя на тестовом backend.
  - Power-loss не проверен; process-kill/reopen не заменяет его ([STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md)).
  - Сопоставление ENOTSUP -> «явный отказ» и EINVAL -> «неподтверждённо» — защитное предположение, не подтверждённое локальной man-страницей `fcntl`.
  - Поведение на non-APFS, сетевых и внешних volumes не проверено; на не-macOS durability не объявляется.
  - Задание «flush error до/после потенциального commit, delayed completion, file creation/rename publication; лог порядка операций» из плана выполнено не полностью (нет публикации через rename и лога порядка на реальном host).

## 3. Задачи S9-07…S9-44

| Задача | Статус | Причина |
|---|---|---|
| S9-07 | `not_started` | Fault backend и oracle; формально зависит от принятого S9-06, у которого не реализована namespace durability (ISSUE-055) |
| S9-08 | `not_started` | Выбор CoW format требует прототипных замеров по правилам `AGENTS.md`; замеров нет |
| S9-09…S9-12 | `blocked` | Зависят от CoW-формата S9-08 (parser, create/root, write transaction, crash recovery); S9-12 дополнительно от S9-11 |
| S9-13 | `blocked` | Нужно устройство хранения гостя из этапов 5/7 и S9-12 |
| S9-14…S9-22 | `blocked` | Цепочка зависимостей от S9-12 и формата S9-08 (snapshots, branch, leases, reclamation, capacity, CLI, import/export, salvage, migration) |
| S9-23 | `blocked` | Зависит от S9-16 (не начат) |
| S9-24 | `blocked` | Нет единого владельца CPU state ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)); нужны компоненты этапа 4 |
| S9-25 | `blocked` | Нужны реальные устройства, время и I/O state из этапов 5–8 |
| S9-26 | `blocked` | UEFI firmware отсутствует (этап 6/8) |
| S9-27 | `blocked` | vTPM отсутствует |
| S9-28…S9-35 | `blocked` | Требуют компонентов этапов 4–8 и результатов S9-13, S9-24…27 |
| S9-36…S9-38 | `blocked` | Цепочка от S9-35 |
| S9-39 | `blocked` | Требуется отдельное разрешение на стенд power-loss и описанная схема; без него gate остаётся `not_run` |
| S9-40…S9-44 | `blocked` | Зависят от S9-18, S9-31, S9-38, S9-39, S9-41 и не начатых задач |

Дополнительно: namespace-часть S9-06 заблокирована ISSUE-055 и влияет на S9-10, S9-11, S9-29 (публикация root/manifest).

## 4. Gates (план, раздел 8)

| Gate | Статус | Доказательство или недостаток |
|---|---|---|
| G1 — independent storage (S9-03…12) | `partial` | Есть: I/O contract, writer lock, RAW, file-data flush. Нет: namespace durability, fault backend/oracle, CoW, recovery с durable watermark, power-loss |
| G2 — snapshots/tools (S9-14…22) | `not_met` | Не начато |
| G3 — coherent full VM (S9-13, 23…31) | `not_met` | Не начато; блокированы ISSUE-056 и отсутствующие компоненты |
| G4 — identity/integration (S9-32…37) | `not_met` | Не начато |
| G5 — failure qualification (S9-38/39/41) | `not_met` | Есть только unit-тесты sticky reconcile и kill процесса (в S9-04); power-loss не выполнялся |
| G6 — reproducibility (S9-40…44) | `not_met` | Замеров, матрицы и независимого итогового ревью нет |

## 5. Открытые блокеры и записи

- [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed): гарантия durability macOS/APFS не подтверждена. Реализован `F_FULLFSYNC` для данных файла (см. S9-06), но namespace durability, проверенный источник по API и испытания уровней 1–3 отсутствуют; запись ещё описывает состояние до кода и требует актуализации.
- [ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner): у архитектурного CPU state нет единого versioned owner; блокирует S9-24 и full-VM checkpoint.
- Нет принятого CoW-формата и замеров (S9-08).
- Нет устройства хранения гостя (S9-13), UEFI, vTPM (S9-26/27).
- Нет разрешения на power-loss стенд (S9-39).
- S9-04: нет read leases, интеграции с VMHost, запрета изменения parent, busy при export/repair.
- STORAGE_FORMAT.md и ISSUE-055 содержат устаревшие утверждения (§1, §5, §7; «кода file I/O пока нет»).

## 6. Тесты

Команды и результаты на `69e0b02` (2026-11-05):

| Команда | Итог |
|---|---|
| `cargo test -p od-storage` | 36 passed, 0 failed |
| `cargo test -p od-host --lib storage_io` | 19 passed, 0 failed |
| `cargo test -p od-host --lib raw_file_tests` | 9 passed, 0 failed |
| `cargo test -p od-host` | 253 passed, **21 failed** (`s3_20*` и `native_register_v1_corpus::reduce_saved_native_register_v1_case`) |

21 падение в полном `cargo test -p od-host` существовало до этапа 9 (по сведениям из задания, проверено на чистом HEAD до S9); в этой сессии падения воспроизведены на текущем состоянии, повторная проверка на чистом HEAD не выполнялась. Они не относятся к коду S9-03…06 и не закрыты. Тесты S9 на этих падениях не основаны.

Что тесты не доказывают: power-loss, namespace durability, поведение на других volumes и в реальных VMHost.
