# Этап 9. Отчёт: надёжность данных и управление состоянием

Статус отчёта: **промежуточный. Этап 9 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и тестами ветки `feat/s9-stage` на коммите `33440be`. Gates G1–G6 не закрыты.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md), [VM_STATE.md](../specs/VM_STATE.md), [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md), [реестр проблем](../KNOWN_ISSUES.md), [SUPPORT_MATRIX.md](../specs/SUPPORT_MATRIX.md).

## 1. Сводка

Всего задач S9-00…S9-44: 45.

| Статус | Количество | Задачи |
|---|---|---|
| Принято (документы, архитектурное ревью 2026-11-05; S9-00 и S9-02 с оговорками) | 3 | S9-00, S9-01, S9-02 |
| Реализовано, ревью пройдено (по сведениям из заданий на отчёт) | 10 | S9-03, S9-05, S9-07, S9-09, S9-10, S9-11, S9-12, S9-14, S9-15, S9-16 |
| Протокол принят с оговорками (размеры блока и карты — гипотезы); формат НЕ принят как нормативный | 1 | S9-08 |
| Частично | 3 | S9-04, S9-06 (код, с оговорками ниже); S9-40 (измерена только commit-latency) |
| `not_started` | 6 | S9-17…S9-22 (зависимость S9-16 выполнена; сами задачи не начаты) |
| `blocked` | 22 | S9-13, S9-23…S9-39, S9-41…S9-44 |

Принятого этапа или принятого gate нет. «Реализовано, ревью пройдено» относится к самой задаче и не означает принятие gate G1. Формат CoW не принят как нормативный: нормативных bytes и лимитов нет, S9-40 выполнен частично. «Ревью пройдено» у S9-09…S9-12 не делает формат принятым. ISSUE-057 закрыт (`01ef35d`). Чекбоксы `[ ]` в плане не отмечены.

## 2. Задачи S9-00…S9-16 и S9-40

### S9-00. Область поддержки и failure model — принято с оговорками

- Коммит: `d773cc2`.
- Файлы: [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md), [VM_STATE.md](../specs/VM_STATE.md), [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md), [KNOWN_ISSUES.md](../KNOWN_ISSUES.md).
- Проверки: только документальные; кода в коммите нет.
- Ревью: принято с оговорками после правок документов (STORAGE_FORMAT.md, ISSUE-055, STORAGE_RECOVERY.md). НЕ разрешены: лимит logical size RAW (в `raw.rs` явного лимита нет — открытый вопрос), лимиты virtual capacity CoW, глубины цепочек, числа snapshots, LBA/CoW block size ([STORAGE_FORMAT §4](../specs/STORAGE_FORMAT.md#4-единицы-размеры-и-лимиты)). Первая host/filesystem/media комбинация (macOS 26.7.1, APFS, внутренний SSD) только предложена и не испытана.
- Расхождения STORAGE_FORMAT.md и ISSUE-055 с кодом после S9-03…06 устранены (`e9fe0ab`, затем правки по архитектурному ревью).
- Архитектурное ревью S9-00…02 (2026-11-05): S9-00 — принято с оговорками (поправлены шапка, §1–§2, §5, §7–§9, §12 STORAGE_FORMAT.md, ISSUE-055, STORAGE_RECOVERY.md); S9-01 — принято после переписывания §2 VM_STATE.md и сверки с кодом HEAD `4dedff2`; S9-02 — принято с оговорками (формулировка «не старше последнего `durable_ack`» в VM_STATE.md §3.3–§3.4). Оговорки: лимит logical size RAW не задан (открытый вопрос), ENOTSUP/EINVAL — предположение.

### S9-01. Prerequisites по фактическому коду — принято

- Коммит: `d773cc2`.
- Файл: [VM_STATE.md §2](../specs/VM_STATE.md#2-инвентаризация-prerequisites-s9-01).
- Проверки: инвентаризация по коду на 2026-11-05. Ревью: первичная редакция §2.1 описывала `od-storage` как scaffold и `od-host` без file I/O; §2 переписан по коду, сверка с HEAD `4dedff2` выполнена, S9-01 принято. ISSUE-057 в prototype CoW закрыт; следующий срез — недостающие замеры S9-40 (VM_STATE.md §2.5). Инвентаризация описывает код на момент сверки и требует обновления при изменениях.
- НЕ доказано: полнота инвентаризации не проверена независимо. Full-VM prerequisites заблокированы ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)).

### S9-02. Операции, состояния, результаты — принято с оговорками

- Коммит: `d773cc2`.
- Файл: [VM_STATE.md §3](../specs/VM_STATE.md#3-операции-и-состояния-s9-02); методика [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md).
- Проверки: документальные; ревью: принято с оговорками (формулировка durability forced stop приведена к «не старше последнего `durable_ack`»). Операции VM (lifecycle, checkpoint, restore, clone) не реализованы; реализован только offline storage.
- НЕ доказано: согласованность контракта с реальными реализациями; открытые вопросы — [VM_STATE.md §4](../specs/VM_STATE.md#4-открытые-вопросы).

### S9-03. Узкий storage I/O interface — реализовано, ревью пройдено

- Коммит: `a59338a`.
- Файлы: [io.rs](../../crates/od-storage/src/io.rs), [io_tests.rs](../../crates/od-storage/src/io_tests.rs), [testing/contract.rs](../../crates/od-storage/src/testing/contract.rs), [testing/memory.rs](../../crates/od-storage/src/testing/memory.rs), [storage_io.rs](../../crates/od-host/src/storage_io.rs).
- Факт: `StorageBackend` и `CheckedStorage` (checked offsets, ограниченный retry short transfer и interrupt, типизированные ошибки), `DurableAck` только при объявленной durability и успешном flush. Общий contract suite исполняется на memory-backend и host file backend (Unix).
- Проверки: `memory_backend_passes_contract*`, `host_file_backend_passes_contract`, тесты overflow, short/zero progress, interrupts, inconsistent capabilities, `rejected_calls_do_not_reach_the_backend`.
- НЕ доказано: поведение на не-Unix платформах (host backend Unix-only); non-APFS и сетевые volumes.

### S9-04. Единственный writer — частично

- Коммиты: `ba77773` (writer lock), `31b8c2a` (read lease), `bc7b675` (busy-политика export/repair, `ParentLease`).
- Файлы: [storage_io.rs](../../crates/od-host/src/storage_io.rs), [io.rs](../../crates/od-storage/src/io.rs).
- Сделано: `open_read_write_exclusive` и `create_new_exclusive` берут `flock(LOCK_EX|LOCK_NB)` на том же fd, что используется для записи; identity `(dev, ino)` проверяется после lock и перед каждой мутацией (`WriterIdentityLost`).
- Проверки: `second_exclusive_open_is_busy_until_first_is_dropped`, `symlink_and_hard_link_aliases_hit_the_same_lock`, `writer_in_another_process_blocks_us_and_crash_releases_it` (в т.ч. SIGKILL, перезапуск тестового бинарника), `writer_in_this_process_blocks_another_process`, `replaced_path_refuses_writes_instead_of_touching_either_file`, `unlinked_path_refuses_writes`, `read_only_open_neither_blocks_nor_is_blocked_by_writer`.
- Сделано (`31b8c2a`): `ReadLease` — shared `flock(LOCK_SH|LOCK_NB)` на read-only дескрипторе с проверкой, что заблокированный inode остаётся тем, что по пути; конфликтует с exclusive writer в обе стороны, мутаций не предоставляет, несколько lease сосуществуют; writer и lease используют один nonblocking helper `flock` с повтором при EINTR. Проверки: `lease_blocks_writer_in_this_process_until_dropped`, `lease_is_busy_while_writer_holds_the_lock_and_works_after`, `several_leases_coexist_and_writer_waits_for_the_last`, `lease_refuses_every_mutation`, `lease_refuses_path_that_names_another_or_no_inode`, `lease_in_another_process_blocks_writer_allows_readers_and_sigkill_releases_it`, `writer_in_this_process_makes_lease_busy_in_another_process`. `HostFileStorage::open_read_only` без lease сохранён и документирован как не-lease (не блокирует и не блокируется).
- Сделано (`bc7b675`, [storage_policy.rs](../../crates/od-host/src/storage_policy.rs)): `export_with_lease` держит read lease на весь проход, `repair_with_writer` — exclusive writer lock; оба сразу отказывают типизированным `Busy` и не вызывают closure при занятом lock. `ParentLease` проверяет identity inode при открытии и перед каждым size/read: подменённый parent даёт `IdentityMismatch`, а не чтение. Проверки (9 тестов `storage_policy`): `export_during_active_writer_is_busy_without_waiting_or_calling_f`, `repair_during_active_lease_or_writer_is_busy_without_calling_f`, `export_lease_is_held_for_the_whole_closure_and_repair_writer_blocks_export`, `repair_is_busy_while_another_process_leases_and_works_after_sigkill`, `hard_link_and_symlink_aliases_share_the_lease_conflict`, `swap_between_check_and_use_is_identity_mismatch_and_never_writes_foreign_file`, `parent_lease_verifies_identity_on_every_use`, `export_does_not_change_file_bytes`.
- НЕ сделано: проверка parent в цепочке (S9-14/16); интеграция с lifecycle VMHost; non-advisory lock на сетевых файловых системах. Сами export и repair (S9-20/21) не реализованы — есть только политика доступа. Read lease на конкретную generation CoW и привязка к immutable root (S9-16/S9-37) не реализованы.
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

### S9-07. Fault backend и независимый oracle — реализовано, ревью пройдено (нет блокеров и мажоров)

- Коммит: `a7874b8`.
- Файлы: [testing/fault.rs](../../crates/od-storage/src/testing/fault.rs), [testing/fault_tests.rs](../../crates/od-storage/src/testing/fault_tests.rs), [testing/rng.rs](../../crates/od-storage/src/testing/rng.rs), [testing/contract.rs](../../crates/od-storage/src/testing/contract.rs), [testing/mod.rs](../../crates/od-storage/src/testing/mod.rs).
- Факт: `FaultBackend` моделирует volatile и durable bytes, pending writes и resize, скриптовые сбои (short, torn по 512-байтовым секторам, I/O errors, потерянные или лживые flush acknowledgements) и seeded crash policies. Независимый oracle на `Vec<u8>` ведёт последний подтверждённый snapshot и допустимые исходы pending; не использует backend и `RawImage`. Seeded-прогоны сверяют каждый шаг с oracle и при сбое печатают seed и operation trace.
- Проверки: `raw_write_flush_crash_reopen_guarantee`, `oracle_detects_ack_without_durability`, `seeded_runs_detect_broken_flush_and_ack`, `seeded_runs_match_oracle`, `run_is_deterministic_for_a_seed`, `lost_ack_outcome_is_old_or_new_and_indistinguishable`, `torn_write_before_flush_is_not_guaranteed_but_flushed_is`, `flush_error_is_sticky_and_not_an_ack`, `resize_error_and_resize_loss`. На момент коммита в `od-storage` было 45 тестов.
- НЕ доказано: модель не включает namespace (rename, create, directory) и реальный power-loss; исходы lost-ack проверяются только как «старое или новое». Реальный накопитель моделью не заменяется (S9-39).

### S9-08. Минимальный собственный CoW format — протокол принят с оговорками; формат НЕ принят как нормативный

- Коммиты: `000c1da` (протокол), `01ef35d` (create заполняет оба слота, `IncompleteCreate`).
- Файлы: [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [cow_proto_tests.rs](../../crates/od-storage/src/cow_proto_tests.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: прототип протокола фиксации (не нормативный формат): два root slot с чётностью generation, копия корня N+1 записывается до первого barrier, `DurableAck` только после второго barrier. Recovery проверяет CRC корней, карты и каждого блока, отказывает при повреждённых зависимостях без молчаливого отката, выполняет roll-forward только от CRC-валидной копии с совпадающими generation и end offset; исход roll-forward — неподтверждённое состояние. Salvage только на чтение.
- Проверки: 1440 прогонов cut-point (`every_cut_point_leaves_old_or_new_never_mixed`, `create_cut_points_give_no_file_or_empty_file`), seeded-прогоны против независимого oracle (`seeded_sequences_match_the_oracle`), отрицательные тесты (`oracle_detects_ack_before_barrier_2`, `oracle_detects_root_without_data_barrier`, `oracle_detects_a_backend_that_acks_without_durability`, `crc_valid_but_hostile_roots_are_refused`, `damaged_data_under_valid_root_refuses_without_fallback`, `both_slots_damaged_refuses_to_open`, `generation_exhaustion_is_rejected_before_any_io`).
- НЕ сделано / НЕ доказано:
  - Размер блока, раскладка корня, плоская карта и все лимиты — гипотезы; S9-40 выполнен частично (только commit-latency, см. S9-40 ниже). Нормативных bytes, root selection как спецификации, обработки unreachable garbage и feature bits нет; [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md#9-требования-к-cow-формату-bytes--tbd-s9-08) остаётся TBD.
  - Не реализованы GC, refcount, snapshots, шифрование (S9-14 и далее). Строгий parser добавлен отдельно (S9-09, ниже).
  - Защита построена на модели отказов `FaultBackend`; реальный power-loss и namespace durability (публикация файла, ISSUE-055) не проверены.
  - Критерий «Готово» плана (нормативные bytes и лимиты определены) не выполнен.

### S9-09. Строгий read-only parser — реализовано, ревью пройдено (нет блокеров и мажоров; minor в сообщении коммита)

- Коммит: `17c9ce6`.
- Файлы: [cow_parse.rs](../../crates/od-storage/src/cow_parse.rs), [cow_parse_tests.rs](../../crates/od-storage/src/cow_parse_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `cow_parse` разбирает слоты, карту блоков и дескрипторы без записи и мутаций backend. Все смещения и длины — checked arithmetic, лимиты проверяются до allocation, ошибки несут секцию, абсолютный offset и тип. Feature words лежат в padding слота: неизвестный mandatory bit запрещает writable open и read-only open (если не объявлен понятым); reserved bytes обязаны быть нулевыми.
- Проверки: литеральные offsets и независимый CRC, каждая точка усечения, overflow, duplicate и overlap, 80000 детерминированных мутаций (fuzz) без паник и с независимой проверкой каждого `Ok`; на момент коммита 76 тестов. Проверки cycle и depth не применимы: ссылок между metadata в прототипном формате нет.
- НЕ доказано: fuzz проверяет soundness, не полноту; параметры формата — гипотезы (формат не принят, S9-40); parser не отражает нормативную спецификацию, которой нет.

### S9-10. Create/open и выбор корня — реализовано, ревью пройдено (после одного major, исправлен); ISSUE-057 закрыт

- Коммиты: `61a99e0`, `88b9f42` (регистрация модуля).
- Файлы: [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [cow_open_tests.rs](../../crates/od-storage/src/cow_open_tests.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `CowFile::open` проверяет всю metadata через строгий parser, writable или read-only; неизвестные mandatory features отклоняются. Выбор корня — по максимальному валидному generation; torn write считается повреждённым слотом; hostile и конфликтующие корни (`GenerationConflict`) отклоняются; молчаливого отката на старый корень нет. При пустом втором слоте на generation 1 с непустым хвостом проверяется копия корня N+1 и выполняется roll-forward. Create пишет generation 1 с одним barrier до ack.
- Проверки: 90 тестов `od-storage` на момент коммита, включая sweep по create и публикациям против независимого oracle.
- Исправление ISSUE-057 (`01ef35d`, [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs)): `create` пишет два пустых корня (поколение 1 в slot 1 и поколение 2 в slot 0) под одним барьером; после ack пустой слот — повреждение; спекулятивная ветка «первый commit» удалена; один валидный пустой корень при файле размером с заголовок — `IncompleteCreate` (writable open отклоняется); повреждённый новейший слот вместе с повреждённой копией корня — `DamagedOtherSlot`, salvage даёт предыдущее поколение с `possible_lost_commit`. Тесты: `create_makes_two_empty_roots_under_one_barrier`, `power_cut_at_every_create_step`, `every_sector_state_of_an_unfinished_create`, `damage_to_one_or_two_root_sectors_never_yields_an_older_writable_state`; sweeps против независимого oracle; `od-storage` — 114 passed. Запись: [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) (закрыто).
- НЕ доказано: реальный power loss; формат не принят как нормативный (S9-40).

### S9-11. CoW write transaction — реализовано, ревью пройдено (один major исправлен)

- Коммит: `7d23bf9`.
- Файлы: [cow_tx.rs](../../crates/od-storage/src/cow_tx.rs), [cow_tx_tests.rs](../../crates/od-storage/src/cow_tx_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `CowTx` — запись с гранулярностью в байт (read-modify-write через новые блоки), несколько extents в одном commit, видимых только после публикации; лимит `MAX_TX_BLOCKS` проверяется до любого вызова backend; отмена. Отменённый commit обрезает хвост и закрепляет откат barrier до ответа `Cancelled`; сбой отката отравляет сессию и даёт `Uncertain`. После `Uncertain` повтор отклоняется без обращения к backend. Публичного пути ack до второго barrier нет (`commit_with_order` только под `#[cfg(test)]`).
- Проверки: `every_cut_of_a_transaction_gives_whole_old_or_whole_new`, `random_ranges_match_the_byte_model`, `partial_block_overwrite_is_copy_on_write_and_matches_the_byte_model`, `several_extents_in_one_commit_appear_together`, `transaction_size_limit_is_checked_before_any_backend_call`, `published_objects_are_immutable_and_the_old_generation_stays_usable`, `ack_follows_data_barrier_root_barrier_in_the_trace`, `a_broken_order_is_caught_by_the_ack_durability_check`, `failed_commit_is_uncertain_and_cannot_be_retried_without_reopen`, `failed_rollback_of_a_cancelled_commit_is_uncertain`, тесты отмены. Major ревью (откат хвоста без barrier) исправлен; покрывающий тест падает без barrier.
- НЕ доказано: реальный power loss, namespace durability, reclaim/GC, snapshots; формат не принят как нормативный (S9-40).

### S9-12. Crash recovery по durable watermark — реализовано (recovery tests), ревью пройдено

- Коммит: `7d23bf9`.
- Файлы: [cow_recovery_tests.rs](../../crates/od-storage/src/cow_recovery_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs).
- Факт и проверки: idempotent open (закоммиченное состояние ничего не пишет), repair одним barrier, обрывы во время repair сходятся к одному состоянию (`crash_during_recovery_converges_to_the_same_state`, `repeated_crashed_recoveries_converge`), отложенные записи, повреждённый блок в multi-extent отображении отклоняется без отката (`corrupt_block_of_a_multi_extent_commit_refuses_without_fallback`), неподтверждённый, но закоммиченный корень — допустимый исход, `inspect` не меняет байты, более старый корень с потерянной подтверждённой записью не проходит oracle (`an_older_valid_root_with_a_lost_acknowledged_write_fails_the_oracle`).
- НЕ доказано: real power loss; recovery для reclaim/GC и snapshots; формат не принят как нормативный (S9-40).

### S9-14. Неизменяемые snapshots и цепочки — реализовано, ревью пройдено (единственный major исправлен)

- Коммит: `7188582`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [storage_policy.rs](../../crates/od-host/src/storage_policy.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `SnapFile` хранит snapshots как удерживаемые generations корня внутри CoW-файла и создаёт writable дочерние файлы с `ParentRef` (file identity родителя, snapshot id, generation, число блоков, checksum карты). Чтение: child, затем snapshot, затем предки; дыры и блоки вне диапазона читаются нулями. Глубина цепочки ограничена `MAX_CHAIN_DEPTH = 8` до чтения данных, цикл по file identity отклоняется. Snapshot проверяется до публикации, поэтому успешная публикация не сообщается как сбой (это исправленный major).
- Проверки (`cow_snapshot_tests`): `snapshot_publication_is_whole_or_absent_under_every_fault`, `an_acknowledged_publication_makes_no_further_backend_call_and_survives_reopen`, `child_writes_leave_every_parent_byte_unchanged`, `later_parent_mutations_are_not_visible_to_the_child`, `many_overwrites_never_change_a_snapshot`, `chain_depth_is_bounded`, `a_chain_that_repeats_a_file_identity_is_a_cycle`, `random_chain_matches_a_flat_model_at_every_level`, `replaced_parent_is_an_identity_mismatch`, `tampered_parent_snapshot_map_is_refused`, `every_wrong_field_of_a_parent_reference_refuses_create_and_open`.
- НЕ доказано / предел: production `ParentResolver` нет (только `NoParents` и тестовые resolvers), продуктом цепочка пока не открывается; CRC блоков предков проверяется лениво при чтении; удаление snapshots и reclaim — S9-17; real power loss не проверен; формат не принят (S9-40).

### S9-15. Branch, checkout и disk rollback offline — реализовано, ревью пройдено

- Коммит: `33440be`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs).
- Факт: `branch_from` создаёт новый child из snapshot, не записывая источник и его предков; rollback — это новая ветка от более старого snapshot, текущий файл не меняется. Активный указатель не переключается: для атомарного переключения нужен durable rename ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)).
- Проверки: `a_branch_reproduces_the_snapshot_and_leaves_source_and_siblings_unchanged`, `branch_creation_is_whole_or_absent_under_every_fault`, `rollback_is_a_new_branch_and_the_current_file_stays`, `branch_refusals_write_nothing`, `a_branch_can_be_cut_from_a_closed_file_through_its_snapshot_reference`, `live_writer_refusal_for_branch_and_child_open`.
- НЕ доказано / предел: атомарный активный указатель (checkout как переключение) не реализован; удаление веток и reclaim — S9-17; формат не принят (S9-40).

### S9-16. Проверка parent identities и leases — реализовано, ревью пройдено

- Коммит: `33440be`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs), [storage_policy.rs](../../crates/od-host/src/storage_policy.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs).
- Факт: `SnapFile::open` / `open_gated` проверяют всю цепочку предков (uid, snapshot record, generation, число блоков, end offset, checksum карты, глубина, циклы) до единственной записи — ремонта корня; типизированные отказы для отсутствующего, несовпадающего, цикличного и слишком глубокого parent; writer, держащий parent, даёт busy через host lease resolver (lease-тесты на реальных файлах в `storage_policy`, 15 тестов).
- Проверки: `a_refused_open_does_not_repair_the_childs_damaged_root_slot`, `a_wrong_map_crc_of_an_intermediate_ancestor_refuses_before_any_write`, `missing_parent_refuses_create_and_open`, `replaced_parent_is_an_identity_mismatch`, `a_child_file_cannot_open_without_its_parent_resolver`, `live_writer_refusal_for_branch_and_child_open`.
- НЕ доказано / предел: интеграция с VMHost lifecycle; production resolver; CRC блоков предков проверяется лениво; locks advisory ([S9-04](#s9-04-единственный-writer--частично)).

### Ветка `feat/s9-10-cow-open`: S9-10, S9-11, S9-12 (отдельные черновики)

Ветка `feat/s9-10-cow-open` (worktree `/private/tmp/open_desktop-s9-10`) отходит от `17c9ce6` и содержит иные коммиты S9-10…S9-12, чем `feat/s9-stage` (`61a99e0`, `7d23bf9`). Ветки разошлись: 11 коммитов только в `feat/s9-stage` (включая S9-04 `31b8c2a`, `bc7b675` и исправление ISSUE-057 `01ef35d`), 7 только в `feat/s9-10-cow-open`. Ни одна из веток не объединена с другой, cherry-pick не выполнялся. До синхронизации слайсы cow-open не входят в состояние `feat/s9-stage`.

- **S9-10, `9067be6`:** `CowFile` открывается через строгий парсер (writable и read-only); выбор корня по CRC и generation. Внешнее ревью S9-10 — пройдено (по сведениям задания; сам отчёт ревью в репозитории не проверялся).
- **S9-11, `d2d43c0`:** `CowTx` с `MAX_TX_BLOCKS` (превышение отказывает до записи), исходы `Uncertain` и poisoned. Тесты `cow_tx_tests` (13): каждая граница flush даёт целый старый или целый новый вид; commit error требует reconcile, повтор отказывает; rejected commit не poison и не выполняет I/O; переписывание создаёт новую версию, старый корень читается; частичный блок — read-modify-write. Не покрыто: реальный power loss, namespace durability, reclaim/GC. Внутреннее ревью (Opus) пройдено, внешнее не проводилось.
- **S9-12, `10ebdf5`:** inspect без записи, cleanup хвоста только после проверки ack, `open_acked` отказывает при отставании от подтверждённого состояния. Тесты `cow_recovery_tests` (14): inspect не имеет побочных эффектов и предсказывает результат open; cleanup удаляет только недостижимый хвост и сходится при обрыве; open позади ack отказывает; старый валидный корень при потерянной подтверждённой записи обнаруживается, а не принимается; recovery идемпотентен в каждой точке cut. Не покрыто: реальный power loss; recovery для GC и snapshots. Внутреннее ревью (Opus) пройдено, внешнее не проводилось.

Старые черновики S9-14…S9-16 на этой ветке (`d1427c9` — `cow_chain`; `e18b102` — `cow_branch`; `1f0a889`, `432d07e` — `cow_lease`) заменены реализацией на `feat/s9-stage` (`7188582`, `33440be`, разделы выше): они остаются на `feat/s9-10-cow-open`, не объединены и в приёмку не входят. Внешнее ревью этих черновиков не проводилось.

### S9-40. Измерение стоимости корректных операций — частично

- Файл: [S9_40_COMMIT_LATENCY.md](../testing/S9_40_COMMIT_LATENCY.md) (новый, в рабочем дереве на момент отчёта, не в коммите; код бенчмарка в репозиторий не добавлялся).
- Факт: замер commit-latency через `HostFileStorage` и `fcntl(F_FULLFSYNC)` на одной машине (macOS 26.7.1, Apple M3 Max, внутренний SSD, APFS): 5 прогонов по 300 измеряемых commit на схему, сверка записанных данных после каждого commit. p50: A (RAW-подобная, 1 барьер) 4.05 мс; B (CoW, 1 барьер, только модель стоимости) 4.90 мс; C (CoW, 2 барьера) 8.93 мс; D (3 барьера) 13.00 мс. Цена одного барьера около 4 мс на commit; хвост p99 растёт сильнее медианы (D до 18.86 мс).
- НЕ закрыто: замеры RAW против CoW по throughput, чтению, write amplification, recovery и памяти; независимый повтор в другие дни; другие тома и накопители. Схема B — не безопасный протокол, схема D упрощена (по документу замера). Документ не делает выводов о долговечности. Отчёт S9-40 не принят.

## 3. Задачи S9-13 и S9-17…S9-44 (кроме S9-40 и S9-14…S9-16, см. §2)

| Задача | Статус | Причина |
|---|---|---|
| S9-13 | `blocked` | Нужно устройство хранения гостя из этапов 5/7 и S9-12 |
| S9-17…S9-22 | `not_started` | Зависимость S9-16 выполнена; сами задачи не начаты: удаление и GC, ENOSPC/zero/discard, CLI, import/export, salvage, migration |
| S9-23 | `blocked` | Контракт полного VM checkpoint: нужны S9-02 и S9-16 (выполнены); ISSUE-056 и компоненты этапов 4–8 |
| S9-24 | `blocked` | Нет единого владельца CPU state ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)); нужны компоненты этапа 4 |
| S9-25 | `blocked` | Нужны реальные устройства, время и I/O state из этапов 5–8 |
| S9-26 | `blocked` | UEFI firmware отсутствует (этап 6/8) |
| S9-27 | `blocked` | vTPM отсутствует |
| S9-28…S9-35 | `blocked` | Требуют компонентов этапов 4–8 и результатов S9-13, S9-24…27 |
| S9-36…S9-38 | `blocked` | Цепочка от S9-35 |
| S9-39 | `blocked` | Требуется отдельное разрешение на стенд power-loss и описанная схема; без него gate остаётся `not_run` |
| S9-41…S9-44 | `blocked` | Зависят от S9-38, S9-39, S9-42 и не начатых задач (S9-40 — частично, см. §2) |

Для S9-07…S9-12 в таблице строк нет: они описаны в §2. Дополнительно: namespace-часть S9-06 заблокирована ISSUE-055 и влияет на S9-10, S9-11, S9-29 (публикация root/manifest).

## 4. Gates (план, раздел 8)

| Gate | Статус | Доказательство или недостаток |
|---|---|---|
| G1 — independent storage (S9-03…12) | `partial` | Есть: I/O contract, writer lock, RAW, file-data flush, fault backend с независимым oracle (S9-07), прототип CoW-протокола (S9-08), строгий parser (S9-09), open и выбор корня (S9-10), write transaction (S9-11), recovery tests (S9-12) на модели отказов. Нет: принятого нормативного формата (S9-08, S9-40 выполнен частично); namespace durability, проверки parent в цепочке и интеграции с VMHost (S9-04), реального power-loss; S9-09…12 реализованы на прототипном формате; ветка `feat/s9-10-cow-open` с отдельными черновиками не объединена. G1 остаётся `partial` |
| G2 — snapshots/tools (S9-14…22) | `not_met` | Есть: immutable snapshots и дочерние цепочки (S9-14, `7188582`), branch и rollback как новая ветка (S9-15, `33440be`), проверка цепочки предков и leases до записи (S9-16, `33440be`). Gate требует S9-14…22 целиком; не начаты: удаление и reclaim (S9-17), capacity/zero/discard (S9-18), CLI (S9-19), import/export (S9-20), salvage (S9-21), migration (S9-22). Нет production `ParentResolver`, атомарного активного указателя (ISSUE-055) и принятого формата |
| G3 — coherent full VM (S9-13, 23…31) | `not_met` | Не начато; блокированы ISSUE-056 и отсутствующие компоненты |
| G4 — identity/integration (S9-32…37) | `not_met` | Не начато |
| G5 — failure qualification (S9-38/39/41) | `partial` | Есть deterministic fault model и seeded-прогоны для RAW и CoW (S9-07/08/10/11/12), cut-point sweeps, repeated crashed recovery на модели, unit-тесты sticky reconcile и kill процесса (S9-04). Нет: полной fault/corruption matrix (S9-38), реального power-loss (S9-39), soak (S9-41) |
| G6 — reproducibility (S9-40…44) | `not_met` | Замеров, матрицы и независимого итогового ревью нет |

## 5. Открытые блокеры и записи

- [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed): гарантия durability macOS/APFS не подтверждена. Реализован `F_FULLFSYNC` для данных файла (см. S9-06), но namespace durability, проверенный источник по API и испытания уровней 1–3 отсутствуют; запись ещё описывает состояние до кода и требует актуализации.
- [ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner): у архитектурного CPU state нет единого versioned owner; блокирует S9-24 и full-VM checkpoint.
- Нет принятого CoW-формата и полных замеров (S9-08, S9-40): есть протокол, parser, open, write transaction, recovery tests, snapshots и цепочки; блокирует приёмку G1/G2 и S9-13 далее.
- Нет production `ParentResolver`: цепочку snapshots продуктом пока открыть нельзя; нет атомарного активного указателя ветки (нужен durable rename, ISSUE-055); CRC блоков предков проверяется лениво.
- Ветки разошлись: `feat/s9-stage` и `feat/s9-10-cow-open` содержат разные коммиты S9-10…S9-12 (общий предок `17c9ce6`). Требуется синхронизация перед приёмкой G1; до неё кросс-ветка состояние не считается единым.
- [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable): закрыт (`01ef35d`); остаётся как справка по решению (create заполняет оба слота).
- Нет устройства хранения гостя (S9-13), UEFI, vTPM (S9-26/27).
- Нет разрешения на power-loss стенд (S9-39).
- S9-04: read lease (`31b8c2a`), busy-политика export/repair и `ParentLease` (`bc7b675`) реализованы; нет проверки parent в цепочке (S9-14/16), интеграции с VMHost lifecycle и non-advisory lock на сетевых ФС; locks advisory.
- Пункты 1–3 расхождений документов с кодом (STORAGE_FORMAT.md шапка, §5, §7; ISSUE-055) закрыты; прочие пункты архитектурного ревью S9-00…02 применены в документах (см. S9-00…02 выше). Остаются: [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed) (namespace, power-loss, ENOTSUP/EINVAL), лимит logical size RAW, §9 STORAGE_FORMAT.md остаётся прототипным.

## 6. Тесты

Команды и результаты: `feat/s9-stage` на `33440be` (2026-11-05) и `feat/s9-10-cow-open` на `432d07e` (проверено 2026-11-05 в `/private/tmp/open_desktop-s9-10`).

| Команда | Итог |
|---|---|
| `cargo test -p od-storage` | 143 passed, 0 failed на `33440be` (36 до S9-07/08; 45 на `a7874b8`; 59 на `000c1da`; 90 на `88b9f42`; 113 на `7d23bf9`) |
| `cargo test -p od-storage` на `feat/s9-10-cow-open` (`432d07e`) | 153 passed, 0 failed: cow_parse 17, cow_proto 1 + cow_proto_tests 17, cow_tx 13, cow_recovery 14, cow_chain 21, cow_branch 10, cow_lease 15, io 23, raw 13, testing 9 |
| `cargo test -p od-host --lib storage_io` | 30 passed, 0 failed (19 до `31b8c2a`) |
| `cargo test -p od-host --lib storage_policy` | 15 passed, 0 failed (9 на `bc7b675`; 15 после lease-тестов S9-14…16) |
| `cargo test -p od-host --lib raw_file_tests` | 9 passed, 0 failed |
| `cargo test -p od-host` | 253 passed, **21 failed** (`s3_20*` и `native_register_v1_corpus::reduce_saved_native_register_v1_case`) |

21 падение в полном `cargo test -p od-host` существовало до этапа 9 (по сведениям из задания, проверено на чистом HEAD до S9); в этой сессии падения воспроизведены на текущем состоянии, повторная проверка на чистом HEAD не выполнялась. Они не относятся к коду S9-03…06 и не закрыты. Тесты S9 на этих падениях не основаны.

Что тесты не доказывают: power-loss, namespace durability, поведение на других volumes и в реальных VMHost; результаты на `FaultBackend` не заменяют испытаний реального носителя.
