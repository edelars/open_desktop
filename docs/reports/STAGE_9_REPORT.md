# Этап 9. Отчёт: надёжность данных и управление состоянием

Статус отчёта: **промежуточный. Этап 9 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и тестами. Разделы до 2026-11-06 написаны по ветке `feat/s9-stage` (до `e57646c`); с 2026-11-18 отчёт ведётся на ветке `codex` (база `92ca05c`, срезы до итогового ревью S9-43). При расхождении приоритет у §10 («Итоговое ревью S9-43 и актуализация 2026-11-18»), затем у §9. Gates G1–G6 не закрыты.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md), [VM_STATE.md](../specs/VM_STATE.md), [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md), [реестр проблем](../KNOWN_ISSUES.md), [SUPPORT_MATRIX.md](../specs/SUPPORT_MATRIX.md).

## 1. Сводка

Всего задач S9-00…S9-44: 45.

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 26 | Решения владельца 2026-11-18. По итогам итогового ревью S9-43 в реализованном объёме: S9-00, S9-01, S9-02, S9-03, S9-05, S9-07, S9-08, S9-09, S9-10, S9-11, S9-12, S9-14, S9-15, S9-16, S9-17, S9-18, S9-19, S9-20, S9-21, S9-41, S9-42, S9-44 (S9-44 — как задача записи приёмки; сам этап не принят). С суженным объёмом: S9-39 — принята (решение владельца 2026-11-18) в объёме USB-накопителя варианта A: 30 прогонов (§7 и §8.7), барьерная семантика `F_FULLFSYNC` не доказана (все отключения позже 2 с), внутренний SSD целевой комбинации не испытан. S9-22 — закрыта в объёме CoW v1 (решение владельца 2026-11-18): путь RAW → v1 через `import`, таблица совместимости; миграции между версиями CoW нет, второй версии нет. S9-06 — закрыта в суженном объёме v1 (решение владельца 2026-11-18): durable-гарантия только для данных файла (`F_FULLFSYNC`); создание и публикация файлов — завершение без durable-гарантии ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed) остаётся пределом). S9-38 — закрыта в объёме storage (решение владельца 2026-11-18): матрица уровней 0 и 1; VM-часть остаётся заблокированной |
| Выполнено, ревью засчитано (решение владельца 2026-11-18), этап не принят | 1 | S9-43 — итоговое ревью storage-scope Claude Opus high засчитано вместо `gpt-6.1-sol` (§10) |
| Частично | 2 | S9-04 (код, с оговорками ниже); S9-40 (commit-latency, throughput/WA/чтение/open/память, цепочки, export, compaction измерены, бенчмарк в репозитории; checkpoint/restore заблокированы S9-29…31) |
| `blocked` | 16 | S9-13, S9-23…S9-37 |

Принятого этапа или принятого gate нет. «Реализовано, ревью пройдено» относится к самой задаче и не означает принятие gate G1. Формат CoW v1 принят владельцем как нормативный 2026-11-06 ([STORAGE_FORMAT §9](../specs/STORAGE_FORMAT.md#9-cow-v1-нормативно-с-2026-11-06)); слой ODSNAP не нормативен (STORAGE_FORMAT §9.8). S9-40 частичный: checkpoint/restore заблокированы S9-29…31. Принятие формата не делает принятыми задачи S9-08…S9-12. ISSUE-057 закрыт (`01ef35d`). Чекбоксы в плане отмечены у 25 принятых задач (решения владельца 2026-11-18); у S9-44 чекбокс «Этап принят» не отмечен: этап не принят.

## 2. Задачи S9-00…S9-22, S9-40 и S9-41

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
- Сделано (2026-11-18, [persistent_nvme.rs](../../crates/od-host/src/persistent_nvme.rs)): тот же lock на публичном пути подключения диска `attach_persistent_nvme` → `PersistentNvmeBackend::open` (вне тестов его пока никто не вызывает; стенд S5 [s5_nvme_stand.rs](../../tools/runner/src/s5_nvme_stand.rs) идёт через `create_new_exclusive` + `attach_persistent_nvme_storage` — тот же `flock`). Тест `a_second_process_is_busy_until_the_writer_process_is_killed`: дочерний процесс открывает backing через `PersistentNvmeBackend::open` и подтверждает запись сектора; пока он жив, два открытия из другого процесса получают `WriterOpen(WriterBusy)`; после SIGKILL (проверяется, что ребёнок завершён именно сигналом SIGKILL: без деструктора и unlock) открытие проходит, подтверждённый сектор читается, нетронутый не изменился, а следующий writer снова busy. Уровень — аварийное завершение процесса (кэши host живы), не power loss. `cargo test -p od-host --lib persistent_nvme`: 19 passed.
- Parent в цепочке: uid проверяет `SnapFile::open`, identity файла — `ParentLease` (S9-16); production `ParentResolver` есть только в офлайн-CLI: `PathResolver` с `ParentLease` в [tools/image-tool](../../tools/image-tool/src/lib.rs) (`1590c9e`); VMHost цепочку snapshots не открывает.
- НЕ сделано: интеграция с lifecycle VMHost — заблокирована, VMHost пока не подключает гостю диски ([Lifecycle.swift](../../apps/vmhost/Sources/Lifecycle.swift), профили без дисков). Non-advisory lock на сетевых файловых системах не предоставляется: сетевые ФС вне гарантий первой версии (план §1, [STORAGE_FORMAT §3](../specs/STORAGE_FORMAT.md)). Export/import (S9-20) и salvage (S9-21) реализованы в `fd5ebcf`; repair-подкоманды нет, есть только политика доступа. Read lease на конкретную generation CoW и привязка к immutable root (S9-16/S9-37) не реализованы.
- НЕ доказано: lock advisory (known limit из коммита `4fcd5c4`): процесс, не вызывающий `flock`, может изменить файл; поведение на сетевых и non-APFS volumes. Критерий «Готово» плана («две VMHost не могут писать один backing») для реальных VMHost не проверен.

### S9-05. RAW backend — реализовано, ревью пройдено

- Коммит: `4fcd5c4`.
- Файлы: [raw.rs](../../crates/od-storage/src/raw.rs), [raw_file_tests.rs](../../crates/od-host/src/raw_file_tests.rs).
- Факт: `RawImage` с фиксированными logical size и block size; `create` не усекает и не перезаписывает существующий файл; read/write проверяются на границы до обращения к backend; короткий файл даёт `IncompleteImage`/`Truncated`, а не нули.
- Проверки: 13 unit-тестов `raw::tests` и 9 `raw_file_tests` (create exact size, refuse existing, reopen с независимым сравнением байтов, truncated-behind-open, range checks, read-only reopen, busy second writer).
- НЕ доказано: power-loss durability (flush на уровне образа см. S9-06); known limit — shrink файла процессом без `flock` между проверкой размера и записью.

### S9-06. Durability от storage до host — закрыта в суженном объёме v1 (решение владельца 2026-11-18)

- Коммит: `69e0b02`.
- Файлы: [storage_io.rs](../../crates/od-host/src/storage_io.rs), [io.rs](../../crates/od-storage/src/io.rs), [io_tests.rs](../../crates/od-storage/src/io_tests.rs), [raw_file_tests.rs](../../crates/od-host/src/raw_file_tests.rs).
- Сделано: **только durability данных файла** на macOS через `fcntl(F_FULLFSYNC)` на заблокированном writable дескрипторе; `DurableAck` создаётся в checked-слое только после успеха backend. Явный отказ (ENOTSUP) даёт `UnsupportedDurability` без смены состояния; любой иной сбой flush, включая `EINVAL`, переводит `CheckedStorage` в sticky reconcile-required (запись, resize, flush отклоняются без обращения к backend; `into_backend` отказывает).
- Проверки: `failed_flush_makes_storage_reconcile_required_and_sticky`, `explicit_durability_refusal_is_not_an_ack_and_not_sticky`, `contradicting_flush_is_also_reconcile_required`, `claimed_durability_must_be_backed_by_the_backend`, `sticky_storage_does_not_release_the_backend`, `flush_file_durable_after_write_on_exclusive_writer`, `flush_file_durable_refuses_lost_writer_identity`, `host_backend_declares_file_durability_only_on_macos_writer_and_lock_only_for_writer`.
- НЕ сделано / НЕ доказано:
  - Namespace durability (create, rename, directory entry) не реализована и остаётся unsupported ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); тесты `failed_namespace_flush_is_sticky` проверяют только поведение checked-слоя на тестовом backend.
  - Power-loss не проверен; process-kill/reopen не заменяет его ([STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md)).
  - Сопоставление ENOTSUP -> «явный отказ» и EINVAL -> «неподтверждённо» — защитное предположение, не подтверждённое локальной man-страницей `fcntl`.
  - Поведение на non-APFS, сетевых и внешних volumes не проверено; на не-macOS durability не объявляется.
  - Задание «flush error до/после потенциального commit, delayed completion, file creation/rename publication; лог порядка операций» выполнено частично: 2026-11-18 журнал порядка commit на реальном host-файле (`a_commit_reaches_the_host_file_in_protocol_order`) и kill процесса на реальных файлах (уровень 1, S9-38); публикации через rename с durability каталога нет (ISSUE-055).

### S9-07. Fault backend и независимый oracle — реализовано, ревью пройдено (нет блокеров и мажоров)

- Коммит: `a7874b8`.
- Файлы: [testing/fault.rs](../../crates/od-storage/src/testing/fault.rs), [testing/fault_tests.rs](../../crates/od-storage/src/testing/fault_tests.rs), [testing/rng.rs](../../crates/od-storage/src/testing/rng.rs), [testing/contract.rs](../../crates/od-storage/src/testing/contract.rs), [testing/mod.rs](../../crates/od-storage/src/testing/mod.rs).
- Факт: `FaultBackend` моделирует volatile и durable bytes, pending writes и resize, скриптовые сбои (short, torn по 512-байтовым секторам, I/O errors, потерянные или лживые flush acknowledgements) и seeded crash policies. Независимый oracle на `Vec<u8>` ведёт последний подтверждённый snapshot и допустимые исходы pending; не использует backend и `RawImage`. Seeded-прогоны сверяют каждый шаг с oracle и при сбое печатают seed и operation trace.
- Проверки: `raw_write_flush_crash_reopen_guarantee`, `oracle_detects_ack_without_durability`, `seeded_runs_detect_broken_flush_and_ack`, `seeded_runs_match_oracle`, `run_is_deterministic_for_a_seed`, `lost_ack_outcome_is_old_or_new_and_indistinguishable`, `torn_write_before_flush_is_not_guaranteed_but_flushed_is`, `flush_error_is_sticky_and_not_an_ack`, `resize_error_and_resize_loss`. На момент коммита в `od-storage` было 45 тестов.
- НЕ доказано: модель не включает namespace (rename, create, directory) и реальный power-loss; исходы lost-ack проверяются только как «старое или новое». Реальный накопитель моделью не заменяется (S9-39).

### S9-08. Минимальный собственный CoW format — протокол принят с оговорками; формат CoW v1 принят как нормативный

- Коммиты: `000c1da` (протокол), `01ef35d` (create заполняет оба слота, `IncompleteCreate`).
- Файлы: [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [cow_proto_tests.rs](../../crates/od-storage/src/cow_proto_tests.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: протокол фиксации CoW v1 (нормативен с 2026-11-06, STORAGE_FORMAT §9.4): два root slot с чётностью generation, копия корня N+1 записывается до первого barrier, `DurableAck` только после второго barrier. Recovery проверяет CRC корней, карты и каждого блока, отказывает при повреждённых зависимостях без молчаливого отката, выполняет roll-forward только от CRC-валидной копии с совпадающими generation и end offset; исход roll-forward — неподтверждённое состояние. Salvage только на чтение.
- Проверки: 1440 прогонов cut-point (`every_cut_point_leaves_old_or_new_never_mixed`, `create_cut_points_give_no_file_or_empty_file`), seeded-прогоны против независимого oracle (`seeded_sequences_match_the_oracle`), отрицательные тесты (`oracle_detects_ack_before_barrier_2`, `oracle_detects_root_without_data_barrier`, `oracle_detects_a_backend_that_acks_without_durability`, `crc_valid_but_hostile_roots_are_refused`, `damaged_data_under_valid_root_refuses_without_fallback`, `both_slots_damaged_refuses_to_open`, `generation_exhaustion_is_rejected_before_any_io`).
- НЕ сделано / НЕ доказано:
  - Обновлено 2026-11-18: размер блока, раскладка корня, плоская карта, лимиты, протокол коммита, create и выбор корня нормативны в CoW v1 с 2026-11-06 ([STORAGE_FORMAT §9](../specs/STORAGE_FORMAT.md#9-cow-v1-нормативно-с-2026-11-06)); решение по производительности по замерам S9-40 — структуры не менять (S9-40 ниже). Обработка unreachable garbage в живом файле (GC) не реализована.
  - Не реализованы GC, refcount, snapshots, шифрование (S9-14 и далее). Строгий parser добавлен отдельно (S9-09, ниже).
  - Защита построена на модели отказов `FaultBackend`; реальный power-loss и namespace durability (публикация файла, ISSUE-055) не проверены.
  - Критерий «Готово» плана выполнен для слоя коммита: bytes и лимиты v1 определены (§9.1–9.3). Не выполнен для остального: обработки unreachable garbage в живом файле (GC) нет, bytes snapshot-метаданных (ODSNAP) не нормативны (§9.8), правило для ненулевых optional feature-битов в §9.3 прямо не определено (в v1 оба слова нулевые).

### S9-09. Строгий read-only parser — реализовано, ревью пройдено (нет блокеров и мажоров; minor в сообщении коммита)

- Коммит: `17c9ce6`.
- Файлы: [cow_parse.rs](../../crates/od-storage/src/cow_parse.rs), [cow_parse_tests.rs](../../crates/od-storage/src/cow_parse_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `cow_parse` разбирает слоты, карту блоков и дескрипторы без записи и мутаций backend. Все смещения и длины — checked arithmetic, лимиты проверяются до allocation, ошибки несут секцию, абсолютный offset и тип. Feature words лежат в padding слота: неизвестный mandatory bit запрещает writable open и read-only open (если не объявлен понятым); reserved bytes обязаны быть нулевыми.
- Проверки: литеральные offsets и независимый CRC, каждая точка усечения, overflow, duplicate и overlap, 80000 детерминированных мутаций (fuzz) без паник и с независимой проверкой каждого `Ok`; на момент коммита 76 тестов. Проверки cycle и depth не применимы: ссылок между metadata в прототипном формате нет.
- НЕ доказано: fuzz проверяет soundness, не полноту; parser написан до принятия формата CoW v1 (2026-11-06); явной трассировки требований §9 STORAGE_FORMAT к тестам parser нет.

### S9-10. Create/open и выбор корня — реализовано, ревью пройдено (после одного major, исправлен); ISSUE-057 закрыт

- Коммиты: `61a99e0`, `88b9f42` (регистрация модуля).
- Файлы: [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [cow_open_tests.rs](../../crates/od-storage/src/cow_open_tests.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `CowFile::open` проверяет всю metadata через строгий parser, writable или read-only; неизвестные mandatory features отклоняются. Выбор корня — по максимальному валидному generation; torn write считается повреждённым слотом; hostile и конфликтующие корни (`GenerationConflict`) отклоняются; молчаливого отката на старый корень нет. При пустом втором слоте на generation 1 с непустым хвостом проверяется копия корня N+1 и выполняется roll-forward. Create пишет generation 1 с одним barrier до ack.
- Проверки: 90 тестов `od-storage` на момент коммита, включая sweep по create и публикациям против независимого oracle.
- Исправление ISSUE-057 (`01ef35d`, [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs)): `create` пишет два пустых корня (поколение 1 в slot 1 и поколение 2 в slot 0) под одним барьером; после ack пустой слот — повреждение; спекулятивная ветка «первый commit» удалена; один валидный пустой корень при файле размером с заголовок — `IncompleteCreate` (writable open отклоняется); повреждённый новейший слот вместе с повреждённой копией корня — `DamagedOtherSlot`, salvage даёт предыдущее поколение с `possible_lost_commit`. Тесты: `create_makes_two_empty_roots_under_one_barrier`, `power_cut_at_every_create_step`, `every_sector_state_of_an_unfinished_create`, `damage_to_one_or_two_root_sectors_never_yields_an_older_writable_state`; sweeps против независимого oracle; `od-storage` — 114 passed. Запись: [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) (закрыто).
- НЕ доказано: реальный power loss; формат CoW v1 принят как нормативный 2026-11-06.

### S9-11. CoW write transaction — реализовано, ревью пройдено (один major исправлен)

- Коммит: `7d23bf9`.
- Файлы: [cow_tx.rs](../../crates/od-storage/src/cow_tx.rs), [cow_tx_tests.rs](../../crates/od-storage/src/cow_tx_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `CowTx` — запись с гранулярностью в байт (read-modify-write через новые блоки), несколько extents в одном commit, видимых только после публикации; лимит `MAX_TX_BLOCKS` проверяется до любого вызова backend; отмена. Отменённый commit обрезает хвост и закрепляет откат barrier до ответа `Cancelled`; сбой отката отравляет сессию и даёт `Uncertain`. После `Uncertain` повтор отклоняется без обращения к backend. Публичного пути ack до второго barrier нет (`commit_with_order` только под `#[cfg(test)]`).
- Проверки: `every_cut_of_a_transaction_gives_whole_old_or_whole_new`, `random_ranges_match_the_byte_model`, `partial_block_overwrite_is_copy_on_write_and_matches_the_byte_model`, `several_extents_in_one_commit_appear_together`, `transaction_size_limit_is_checked_before_any_backend_call`, `published_objects_are_immutable_and_the_old_generation_stays_usable`, `ack_follows_data_barrier_root_barrier_in_the_trace`, `a_broken_order_is_caught_by_the_ack_durability_check`, `failed_commit_is_uncertain_and_cannot_be_retried_without_reopen`, `failed_rollback_of_a_cancelled_commit_is_uncertain`, тесты отмены. Major ревью (откат хвоста без barrier) исправлен; покрывающий тест падает без barrier.
- НЕ доказано: реальный power loss, namespace durability, reclaim/GC, snapshots; формат CoW v1 принят как нормативный 2026-11-06.

### S9-12. Crash recovery по durable watermark — реализовано (recovery tests), ревью пройдено

- Коммит: `7d23bf9`.
- Файлы: [cow_recovery_tests.rs](../../crates/od-storage/src/cow_recovery_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs).
- Факт и проверки: idempotent open (закоммиченное состояние ничего не пишет), repair одним barrier, обрывы во время repair сходятся к одному состоянию (`crash_during_recovery_converges_to_the_same_state`, `repeated_crashed_recoveries_converge`), отложенные записи, повреждённый блок в multi-extent отображении отклоняется без отката (`corrupt_block_of_a_multi_extent_commit_refuses_without_fallback`), неподтверждённый, но закоммиченный корень — допустимый исход, `inspect` не меняет байты, более старый корень с потерянной подтверждённой записью не проходит oracle (`an_older_valid_root_with_a_lost_acknowledged_write_fails_the_oracle`).
- НЕ доказано: real power loss; recovery для reclaim/GC и snapshots; формат CoW v1 принят как нормативный 2026-11-06.

### S9-14. Неизменяемые snapshots и цепочки — реализовано, ревью пройдено (единственный major исправлен)

- Коммит: `7188582`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [storage_policy.rs](../../crates/od-host/src/storage_policy.rs), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: `SnapFile` хранит snapshots как удерживаемые generations корня внутри CoW-файла и создаёт writable дочерние файлы с `ParentRef` (file identity родителя, snapshot id, generation, число блоков, checksum карты). Чтение: child, затем snapshot, затем предки; дыры и блоки вне диапазона читаются нулями. Глубина цепочки ограничена `MAX_CHAIN_DEPTH = 8` до чтения данных, цикл по file identity отклоняется. Snapshot проверяется до публикации, поэтому успешная публикация не сообщается как сбой (это исправленный major).
- Проверки (`cow_snapshot_tests`): `snapshot_publication_is_whole_or_absent_under_every_fault`, `an_acknowledged_publication_makes_no_further_backend_call_and_survives_reopen`, `child_writes_leave_every_parent_byte_unchanged`, `later_parent_mutations_are_not_visible_to_the_child`, `many_overwrites_never_change_a_snapshot`, `chain_depth_is_bounded`, `a_chain_that_repeats_a_file_identity_is_a_cycle`, `random_chain_matches_a_flat_model_at_every_level`, `replaced_parent_is_an_identity_mismatch`, `tampered_parent_snapshot_map_is_refused`, `every_wrong_field_of_a_parent_reference_refuses_create_and_open`.
- НЕ доказано / предел: production `ParentResolver` есть только в офлайн-CLI: `PathResolver` с `ParentLease` в [tools/image-tool](../../tools/image-tool/src/lib.rs) (`1590c9e`); VMHost цепочку snapshots не открывает (обновлено 2026-11-18); CRC блоков предков проверяется лениво при чтении; удаление snapshots и reclaim — S9-17; real power loss не проверен; слой ODSNAP не нормативен (STORAGE_FORMAT §9.8).

### S9-15. Branch, checkout и disk rollback offline — реализовано, ревью пройдено

- Коммит: `33440be`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs).
- Факт: `branch_from` создаёт новый child из snapshot, не записывая источник и его предков; rollback — это новая ветка от более старого snapshot, текущий файл не меняется. Активный указатель не переключается: для атомарного переключения нужен durable rename ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)).
- Проверки: `a_branch_reproduces_the_snapshot_and_leaves_source_and_siblings_unchanged`, `branch_creation_is_whole_or_absent_under_every_fault`, `rollback_is_a_new_branch_and_the_current_file_stays`, `branch_refusals_write_nothing`, `a_branch_can_be_cut_from_a_closed_file_through_its_snapshot_reference`, `live_writer_refusal_for_branch_and_child_open`.
- НЕ доказано / предел: атомарный активный указатель (checkout как переключение) не реализован; удаление веток и reclaim — S9-17; слой ODSNAP не нормативен (STORAGE_FORMAT §9.8).

### S9-16. Проверка parent identities и leases — реализовано, ревью пройдено

- Коммит: `33440be`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs), [storage_policy.rs](../../crates/od-host/src/storage_policy.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs).
- Факт: `SnapFile::open` / `open_gated` проверяют всю цепочку предков (uid, snapshot record, generation, число блоков, end offset, checksum карты, глубина, циклы) до единственной записи — ремонта корня; типизированные отказы для отсутствующего, несовпадающего, цикличного и слишком глубокого parent; writer, держащий parent, даёт busy через host lease resolver (lease-тесты на реальных файлах в `storage_policy`, 15 тестов).
- Проверки: `a_refused_open_does_not_repair_the_childs_damaged_root_slot`, `a_wrong_map_crc_of_an_intermediate_ancestor_refuses_before_any_write`, `missing_parent_refuses_create_and_open`, `replaced_parent_is_an_identity_mismatch`, `a_child_file_cannot_open_without_its_parent_resolver`, `live_writer_refusal_for_branch_and_child_open`.
- НЕ доказано / предел: интеграция с VMHost lifecycle; resolver вне офлайн-CLI (`PathResolver` в image-tool есть, 2026-11-18); CRC блоков предков проверяется лениво; locks advisory ([S9-04](#s9-04-единственный-writer--частично)).

### S9-17. Удаление снимков и reclamation — реализовано, ревью пройдено (read-only; мажоров не было)

- Коммит: `82f72e8`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [storage_policy.rs](../../crates/od-host/src/storage_policy.rs).
- Факт: `delete_snapshot` убирает имя snapshot одним commit, блоки остаются на месте; `rootset` перечисляет каждый блок, достижимый из head, именованных snapshots, обоих восстановимых корней и их копий. `compact_to` записывает новый файл с head и всеми живыми snapshots под флагом building, переоткрывает его с носителя, сравнивает каждый блок с источником и лишь затем снимает флаг; источник не пишется; изменённая или отсутствующая цель не публикуется как завершённая. Host-хелперы берут exclusive writer lock, поэтому открытый child, держащий parent lease, делает delete и compaction занятыми (busy).
- Проверки: `snapshot_delete_is_whole_or_absent_and_never_damages_data_under_every_fault`, `compaction_publishes_a_complete_verified_target_or_nothing_under_every_fault`, `rootset_covers_head_snapshots_both_roots_and_clone_parents`, `the_previous_recoverable_root_still_reads_a_snapshot_deleted_in_the_current_one`, `a_deleted_snapshot_stays_in_the_rootset_while_the_previous_root_names_it`, `compaction_refuses_bad_targets_without_writing_the_source`, `an_unpublished_compaction_target_does_not_open`, `compaction_reproduces_head_and_live_snapshots_in_a_new_smaller_file`, `a_child_of_the_old_file_is_not_reconnected_to_the_compacted_one`; цель проверена переоткрытием с диска. `od-storage`: 156 passed; `storage_policy`: 16 passed.
- НЕ доказано / предел: атомарной подмены источника результатом compaction нет (нужен durable rename, [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); дети удалённого снимка становятся неоткрываемыми (`SnapshotMissing`); GC в живом файле не реализован (только offline compaction в новый файл); флаг building — ненормативное расширение формата; compaction и удаление под конкурирующими читателями не проверены (S9-37); real power loss не проверен; слой ODSNAP не нормативен (STORAGE_FORMAT §9.8). Задача принята владельцем 2026-11-18; gate не объявлен выполненным.

### S9-18. Capacity, explicit zero и discard — реализовано, ревью пройдено (read-only)

- Коммит: `1590c9e`.
- Файлы: [cow_snapshot.rs](../../crates/od-storage/src/cow_snapshot.rs), [cow_snapshot_tests.rs](../../crates/od-storage/src/cow_snapshot_tests.rs), [cow_proto.rs](../../crates/od-storage/src/cow_proto.rs), [io.rs](../../crates/od-storage/src/io.rs), [storage_policy.rs](../../crates/od-host/src/storage_policy.rs).
- Факт: `zero_range` пишет явные нулевые блоки (маскируют parent); `discard_range` снимает присутствие полностью покрытых блоков, и child снова видит parent. Проверки capacity и quota выполняются до любого вызова backend; ENOSPC на каждом шаге commit и compaction не портит предыдущее durable состояние.
- Проверки: `zero_masks_the_parent_and_discard_gives_it_back`, `zero_and_discard_reject_bad_ranges_before_any_backend_call`, `quota_refuses_data_before_any_backend_call_and_the_reserve_serves_metadata`, `enospc_at_every_step_leaves_the_previous_durable_state_intact`, `a_snapshot_keeps_what_it_saw_across_zero_and_discard_and_compaction`, `a_base_file_discard_reads_zeros_and_capabilities_are_honest`, `no_backend_advertises_discard`.
- НЕ доказано / предел: discard не рекламируется гостю; нет hole punching на host; блоки остаются плотными до compaction; ENOSPC и power loss проверены только fault-инъекцией, не на реальном томе; слой ODSNAP не нормативен (STORAGE_FORMAT §9.8). Задача принята владельцем 2026-11-18; gate не объявлен выполненным.

### S9-19. Inspect/check CLI — реализовано, ревью пройдено (read-only)

- Коммит: `1590c9e`.
- Файлы: [cow_inspect.rs](../../crates/od-storage/src/cow_inspect.rs), [cow_inspect_tests.rs](../../crates/od-storage/src/cow_inspect_tests.rs), [tools/image-tool/src/lib.rs](../../tools/image-tool/src/lib.rs), [tools/image-tool/src/main.rs](../../tools/image-tool/src/main.rs), [tools/image-tool/tests/cli.rs](../../tools/image-tool/tests/cli.rs).
- Факт: `inspect_storage` читает через тот же strict parser и ничего не пишет; `od-image-tool` с подкомандами `info` и `check` возвращает коды выхода 0 (ok), 1 (usage), 2 (corrupt), 3 (unsupported), 4 (busy), 5 (io) и печатает ограниченный вывод `key=value` без payload.
- Проверки: `cow_inspect_tests` (в т.ч. `damaged_roots_are_reported_and_never_repaired`, `reports_are_bounded_and_carry_no_payload`, `an_unsupported_mandatory_feature_is_unsupported_not_corrupt`, `a_corrupt_block_is_named_and_a_snapshot_only_block_needs_check_mode`); `od-storage` 175 passed, `storage_policy` 17 passed, `tools/image-tool` 7 passed (проверено запуском).
- НЕ доказано / предел: CLI использует модули CoW под feature `test-support` (в обычную сборку не переведены); слой ODSNAP не нормативен (STORAGE_FORMAT §9.8); нет repair-подкоманды; сценарии busy/io покрыты ограниченно.

### S9-20. RAW export/import с проверкой результата — реализовано, ревью субагентом (без блокеров и мажоров), принято (решение владельца 2026-11-18)

- Коммит: `fd5ebcf`.
- Файлы: [cow_export.rs](../../crates/od-storage/src/cow_export.rs), [cow_export_tests.rs](../../crates/od-storage/src/cow_export_tests.rs), [storage_policy.rs](../../crates/od-host/src/storage_policy.rs), [tools/image-tool/src/lib.rs](../../tools/image-tool/src/lib.rs), [tools/image-tool/tests/cli.rs](../../tools/image-tool/tests/cli.rs).
- Факт: `export_raw` читает один проверенный корень под read lease, пишет в новую цель точного logical size, делает flush и сравнивает каждый байт со вторым чтением источника до признания цели завершённой. `import_raw` строит building-образ и публикует его после переоткрытия. CLI пишет `<out>.partial` и публикует через no-replace hard link; существующие цель или partial-файл отклоняются, partial-файл не удаляется.
- Проверки: `export_matches_the_independent_model_for_head_and_every_snapshot`, `import_then_export_round_trips_and_an_independent_reader_agrees`, `a_fault_at_every_mutating_step_of_the_target_never_reports_a_completed_export`, `an_existing_target_is_refused_and_its_bytes_do_not_change`, `a_source_changed_after_the_reader_fixed_its_root_is_noticed`, `cancellation_at_every_poll_stops_with_cancelled_and_never_succeeds`, `a_corrupt_source_block_fails_the_export_without_a_report`, `a_child_is_flattened_through_its_verified_chain_and_needs_the_parent`; `od-storage` 201 passed, `storage_policy` 20 passed, `tools/image-tool` 16 passed (проверено запуском).
- Ревью: независимое ревью субагентом 2026-11-06 (только чтение, §8): блокеров и мажоров нет; minor открыты. Ранее вручную проверены `publish_no_replace` (не заменяет существующую цель) и правила validity salvage. Остаточный minor: публикация удаляет собственную ссылку при провале проверки identity, что может гоняться с одновременной подменой имени цели. Дефектов в проверенных функциях не найдено, но это не независимое ревью.
- Сделано 2026-11-18: подкоманда CLI `import <in.raw> <new.img>` ([lib.rs](../../tools/image-tool/src/lib.rs), `import_file`): RAW читается под read lease (занятый writer — exit 4), невыровненный или больший `MAX_DISK_BLOCKS` RAW — exit 3 без создания цели, существующее имя — exit 5 `target_exists` без изменения; цель создаётся эксклюзивно, остаётся building до последнего durable commit после переоткрытия и сравнения со свежим чтением RAW; отказ после создания — exit 5 с `target_left=true`. `import_raw` получает цель замыканием `make_target`, которое вызывается только после проверки размера RAW (как `export_raw` и `salvage_to_copy`), и до первой записи отказывает цели без записи/resize (`TargetUnsuitable`) или без file durability (`DurabilityUnavailable`); до правки такая цель получала запись и оставалась building с ошибкой `UnsupportedDurability`. Проверки: `an_import_target_that_cannot_be_durable_is_refused_before_any_write`, `import_refuses_unaligned_oversized_and_existing_targets_without_touching_them` (теперь и «цель не создаётся»), CLI `import_builds_a_new_image_that_exports_back_to_the_same_raw` (RAW → образ → `check` → `export` даёт исходные байты), `import_refuses_unusable_input_busy_input_and_existing_names_without_writing`; `od-storage` 210 passed, `od-image-tool` 21 passed.
- НЕ доказано / предел: нет durable rename ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); удаление `.partial` после успешной публикации export — path-операция ([ISSUE-517](../KNOWN_ISSUES.md#issue-517-s9-20-partial-removal-is-a-path-operation)); import ограничен одним слоем (`MAX_DISK_BLOCKS`); power loss не проверен.

### S9-21. Salvage / recovery-to-copy — реализовано, ревью субагентом (без блокеров и мажоров), принято (решение владельца 2026-11-18)

- Коммит: `fd5ebcf`.
- Файлы: [cow_salvage.rs](../../crates/od-storage/src/cow_salvage.rs), [cow_salvage_tests.rs](../../crates/od-storage/src/cow_salvage_tests.rs), [tools/image-tool/src/lib.rs](../../tools/image-tool/src/lib.rs).
- Факт: `salvage_to_copy` классифицирует диапазоны как valid, lost, unknown или conflicting с указанием основания. Как valid копируются только блоки, на которые ссылается проверенная карта с совпавшим checksum, а также holes и проверенные цепочки parent; устаревшие блоки без provenance как текущие не копируются; неисправимый вход не создаёт цель. CLI завершает с кодом 2 для partial и unrecoverable и копирует partial-результат только с `--accept-partial`.
- Проверки: `an_intact_image_is_complete_and_its_copy_equals_the_model`, `a_damaged_head_block_is_lost_and_the_stale_older_version_is_not_used`, `a_damaged_map_is_unrecoverable_with_a_listing_and_no_copy`, `unordered_roots_that_disagree_are_conflicting_and_neither_version_is_copied`, `a_possible_lost_commit_is_an_older_root_and_says_so`, `a_child_without_its_parent_has_unknown_holes_not_zeros`, `a_target_problem_never_leaves_a_published_copy`, `reports_are_bounded_and_carry_no_payload`.
- Ревью: субагентом 2026-11-06 (§8): блокеров и мажоров нет; minor п. 1 и 5 (§8) исправлены, п. 5 — 2026-11-18 (предварительный отказ цели без durability, ревью Opus high).
- НЕ доказано / предел: Partial копируется только с `--accept-partial`; восстановления конфликтующих roots нет (они перечисляются как conflicting, ни одна версия не копируется); нет durable rename (ISSUE-055); слой ODSNAP не нормативен (STORAGE_FORMAT §9.8).

### S9-41. Repeated recovery и soak — реализовано, ревью субагентом (без блокеров и мажоров), принято (решение владельца 2026-11-18)

- Коммит: `e57646c`.
- Файлы: [cow_soak_tests.rs](../../crates/od-storage/src/cow_soak_tests.rs), [cow_proto_tests.rs](../../crates/od-storage/src/cow_proto_tests.rs).
- Факт: детерминированные soak-прогоны (в тесте по умолчанию 500 циклов на seed, 3 seeds) по записям, snapshots, удалениям, веткам, compaction, export и salvage; сбои на каждом вызове backend, crash policies, порванные root slots, сбои внутри recovery; независимый байтовый oracle проверяет head и каждый snapshot после каждого отказа или crash; проверяются рост и повторное использование generation, snapshot id, ёмкости и числа сессий; исчерпание generation отклоняется до I/O.
- Проверки: `bounded_soak_with_repeated_recovery_has_no_unexplained_loss`, `generation_exhaustion_is_refused_before_io_and_compaction_continues`. С 2026-11-18 `next_snapshot_id` опубликованного заголовка сверяется с оракулом после каждого восстановления (с ожиданием того исхода, который landed) и в конце каждого цикла; значение берётся read-only инспектором только если он прочитал тот же корень, что и writer (generation, file uid, не building) (минор 6 §8).
- Длинный прогон 2026-11-18 (выполнен, scope задан до старта): 8 seeds `0x5041C0DE0101`…`0x5041C0DE0108`, `OD_SOAK_CYCLES=12000` на seed, `cargo test -p od-storage --lib --release`, macOS 26.7.1, Apple M3 Max; база `4c2b8c9` + итоговая проверка `next_snapshot_id` этого среза (только `cow_soak_tests.rs`). Все 8 — `result=ok`, по 6.4–7.0 с на seed. Первый прогон того же scope (база `1e4b96b` + тогда ещё не закоммиченная правка S9-21 и первая версия проверки) дал те же счётчики: прогон детерминирован по seed. Суммарно: 96000 циклов, 22396 crash, 16798 injected faults, 4105 порванных root slots, 27104 open (290 отказанных из-за сбоя внутри recovery), 1831 roll-forward, 19257 проверок старого writer, 2202 compaction, 7863 export, 3877 salvage, 1820 миграций RAW, 42989 веток. Лог: [S9_41_soak_2026-11-18.log](data/stage9/S9_41_soak_2026-11-18.log). Прежние цифры «1500/12000 циклов» оставались по сведениям заданий и этим прогоном заменены.
- НЕ доказано / предел: только модель `FaultBackend`, не реальный диск и не power loss; ограниченный прогон без расхождений — свидетельство для этой модели и параметров, не доказательство; namespace durability не моделируется (ISSUE-055); слой ODSNAP не нормативен (STORAGE_FORMAT §9.8).

### Ветка `feat/s9-10-cow-open`: S9-10, S9-11, S9-12 (отдельные черновики)

Ветка `feat/s9-10-cow-open` (worktree `/private/tmp/open_desktop-s9-10`) отходит от `17c9ce6` и содержит иные коммиты S9-10…S9-12, чем `feat/s9-stage` (`61a99e0`, `7d23bf9`). Ветки разошлись: 11 коммитов только в `feat/s9-stage` (включая S9-04 `31b8c2a`, `bc7b675` и исправление ISSUE-057 `01ef35d`), 7 только в `feat/s9-10-cow-open`. Ни одна из веток не объединена с другой, cherry-pick не выполнялся. До синхронизации слайсы cow-open не входят в состояние `feat/s9-stage`.

- **S9-10, `9067be6`:** `CowFile` открывается через строгий парсер (writable и read-only); выбор корня по CRC и generation. Внешнее ревью S9-10 — пройдено (по сведениям задания; сам отчёт ревью в репозитории не проверялся).
- **S9-11, `d2d43c0`:** `CowTx` с `MAX_TX_BLOCKS` (превышение отказывает до записи), исходы `Uncertain` и poisoned. Тесты `cow_tx_tests` (13): каждая граница flush даёт целый старый или целый новый вид; commit error требует reconcile, повтор отказывает; rejected commit не poison и не выполняет I/O; переписывание создаёт новую версию, старый корень читается; частичный блок — read-modify-write. Не покрыто: реальный power loss, namespace durability, reclaim/GC. Внутреннее ревью (Opus) пройдено, внешнее не проводилось.
- **S9-12, `10ebdf5`:** inspect без записи, cleanup хвоста только после проверки ack, `open_acked` отказывает при отставании от подтверждённого состояния. Тесты `cow_recovery_tests` (14): inspect не имеет побочных эффектов и предсказывает результат open; cleanup удаляет только недостижимый хвост и сходится при обрыве; open позади ack отказывает; старый валидный корень при потерянной подтверждённой записи обнаруживается, а не принимается; recovery идемпотентен в каждой точке cut. Не покрыто: реальный power loss; recovery для GC и snapshots. Внутреннее ревью (Opus) пройдено, внешнее не проводилось.

Старые черновики S9-14…S9-16 на этой ветке (`d1427c9` — `cow_chain`; `e18b102` — `cow_branch`; `1f0a889`, `432d07e` — `cow_lease`) заменены реализацией на `feat/s9-stage` (`7188582`, `33440be`, разделы выше): они остаются на `feat/s9-10-cow-open`, не объединены и в приёмку не входят. Внешнее ревью этих черновиков не проводилось.

### S9-22. Версии формата и migration — закрыта в объёме CoW v1 (решение владельца 2026-11-18)

- Коммит: `b5f75d3`.
- Файлы: [cow_fixture_tests.rs](../../crates/od-storage/src/cow_fixture_tests.rs), [v1_created_committed.img](../../crates/od-storage/tests/fixtures/cow/v1_created_committed.img) (2560 байт), [lib.rs](../../crates/od-storage/src/lib.rs).
- Факт: заморожена фикстура текущего прототипа (создание и один commit); регрессионные тесты читают её по литеральным значениям заголовка, checksum и размера, открывают writable и read-only (salvage) и отклоняют образ с неизвестным mandatory-битом. Фикстура пересоздаётся только явным ignored-тестом.
- Проверки: `frozen_image_layout_is_unchanged`, `frozen_image_opens_writable`, `frozen_image_opens_read_only_salvage`, `unknown_mandatory_feature_is_refused` (+ ignored `write_fixture`); `od-storage` 205 passed, 1 ignored (проверено запуском).
- Сделано: таблица совместимости — [STORAGE_FORMAT §9.7](../specs/STORAGE_FORMAT.md#97-совместимость-и-миграция-s9-22) (v1 читается и пишется, v2+ — типизированный отказ, `cow_parse_tests::identity_version_and_endianness` для version 2 и 0). С 2026-11-18 единственный поддержанный путь обновления в CoW v1 — `import` из RAW в новый target (S9-20): источник не меняется, цель публикуется только после проверки, прерванный import оставляет файл, который не открывается как образ (незавершённый building или файл без валидного заголовка), нехватка места и сбой на каждом мутирующем шаге проверены (`an_import_that_stops_early_leaves_a_target_that_does_not_open`), отказ после publishing commit оставляет полную копию, частичной не бывает.
- НЕ сделано: миграция между версиями CoW — второй версии формата нет, проверять нечего. Критерий «Готово» плана выполнен для пути RAW → v1. Владелец 2026-11-18 решил считать задачу закрытой в объёме v1; миграция между версиями CoW — работа будущей версии формата.

### S9-40. Измерение стоимости корректных операций — частично

- Файл: [S9_40_COMMIT_LATENCY.md](../testing/S9_40_COMMIT_LATENCY.md) (в коммите `e2e6892`; код бенчмарка в репозиторий не добавлялся).
- Факт: замер commit-latency через `HostFileStorage` и `fcntl(F_FULLFSYNC)` на одной машине (macOS 26.7.1, Apple M3 Max, внутренний SSD, APFS): 5 прогонов по 300 измеряемых commit на схему, сверка записанных данных после каждого commit. p50: A (RAW-подобная, 1 барьер) 4.05 мс; B (CoW, 1 барьер, только модель стоимости) 4.90 мс; C (CoW, 2 барьера) 8.93 мс; D (3 барьера) 13.00 мс. Цена одного барьера около 4 мс на commit; хвост p99 растёт сильнее медианы (D до 18.86 мс).
- Throughput, чтение, write amplification, open/roll-forward и память RAW против CoW измерены (`232049b`, [S9_40_THROUGHPUT.md](../testing/S9_40_THROUGHPUT.md); код того замера вне репозитория).
- Сделано 2026-11-18 ([S9_40_CHAIN_EXPORT_COMPACT.md](../testing/S9_40_CHAIN_EXPORT_COMPACT.md)): воспроизводимый бенчмарк в репозитории [`tools/bench/src/bin/s9_40_storage.rs`](../../tools/bench/src/bin/s9_40_storage.rs) (production `HostFileStorage`, все барьеры и проверки, сверка каждого сценария с моделью), 5 прогонов + прогрев, сырые данные [S9_40_storage_2026-11-18.csv](data/stage9/S9_40_storage_2026-11-18.csv). Повтор записи 4 КиБ rand близок к прежнему замеру (на 4–16% выше; WA совпадает, отношение CoW/RAW почти не изменилось — 0.52 против 0.48 при G=32: G=32 — RAW 24.1 / CoW 12.5 MB/s, WA 4.008; G=1 — 0.98 / 0.42 MB/s, WA 97.25). Цепочки: при постоянном объёме (пустые дочерние слои) open верхнего слоя ≈20 мс на глубине 1…8, то есть глубина почти не стоит; при растущем объёме (префиксы, 2.25…11 MiB) open 20.9 / 25.7 / 43.9 / 115.3 мс — около 11 мс на MiB проверяемых данных; полное чтение 266 MB/s у одиночной базы и 136–152 MB/s на любой глубине ≥ 1, включая чтение всего из базы через 8 уровней. `export_raw` 2 MiB — 33.6 мс (d=0) и 139.8 мс (d=8, разница почти целиком — open цепочки); `compact_to` — 272 мс, файл 66.4 → 2.7 МБ.
- Решение (по данным): структуры формата v1 и лимиты v1 и snapshot-слоя не меняются; `MAX_CHAIN_DEPTH = 8` сохраняется, но время open ограничивает объём проверяемой цепочки, а не глубина (гипотеза для реальных размеров диска; следующий эксперимент — после S9-13); полная перезапись карты в commit — известный предел (WA по расчёту линейна по размеру карты), инкрементальная карта — следующий эксперимент при появлении целевых требований гостевого диска (S9-13).
- НЕ закрыто: пауза/длительность checkpoint и restore (заблокированы S9-29…31); фрагментация на длительной нагрузке; масштаб на реальных размерах диска; независимый повтор в другие дни, на других томах и накопителях. Схема B — не безопасный протокол, схема D упрощена (по документу замера). Документы замеров не делают выводов о долговечности.

## 3. Задачи S9-13, S9-23…S9-39 и S9-42…S9-44 (кроме S9-14…S9-22, S9-40, S9-41, см. §2)

| Задача | Статус | Причина |
|---|---|---|
| S9-13 | `blocked` | Нужно устройство хранения гостя из этапов 5/7 и S9-12 |
| S9-23 | `blocked` | Контракт полного VM checkpoint: нужны S9-02 и S9-16 (выполнены); ISSUE-056 и компоненты этапов 4–8 |
| S9-24 | `blocked` | Нет единого владельца CPU state ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)); нужны компоненты этапа 4 |
| S9-25 | `blocked` | Нужны реальные устройства, время и I/O state из этапов 5–8 |
| S9-26 | `blocked` | Нет UEFI variables store; владелец — этап 7 (S7-09, S7-10, S7-33; [план](../plans/STAGE_7.md)); исправлено по [ISSUE-086](../KNOWN_ISSUES.md#issue-086-s9-26-wrong-uefi-stage-reference) |
| S9-27 | `blocked` | vTPM отсутствует |
| S9-28…S9-35 | `blocked` | Требуют компонентов этапов 4–8 и результатов S9-13, S9-24…27 |
| S9-36, S9-37 | `blocked` | Цепочка от S9-35 |
| S9-38 | закрыта в объёме storage (решение владельца 2026-11-18); VM-часть `blocked` | Storage-матрица уровня 0 полная ([STORAGE_RECOVERY.md §4.1](../testing/STORAGE_RECOVERY.md#41-матрица-s9-38-на-уровне-0-модель-faultbackend-2026-11-18)): полный перебор отказов × политик сбоя для RAW, CoW create/commit/repair, `create_base`, write/zero/discard, snapshot, delete, branch, compaction, export, import, salvage; отмена для tx, export, import; fuzz метаданных снимков и отказ writable open на повреждённом файле без записей. Уровень 1 (2026-11-18, [STORAGE_RECOVERY.md §4.2](../testing/STORAGE_RECOVERY.md#42-матрица-s9-38-на-уровне-1-kill-процесса-на-реальных-файлах-2026-11-18)): SIGKILL перед каждым мутирующим вызовом 9 операций на реальных файлах, 169 точек, оба исхода в каждой операции. Не сделано: VM-часть (S9-37, S9-29…31, S9-13) |
| S9-39 | принята (решение владельца 2026-11-18) в объёме USB-накопителя варианта A: 30 прогонов (§7 и §8.7), барьерная семантика `F_FULLFSYNC` не доказана (все отключения позже 2 с), внутренний SSD целевой комбинации не испытан; новый план для точек между барьерами выполнен 2026-11-18 ([§8.7](../testing/S9_39_POWER_LOSS_PLAN.md#87-результаты-2026-11-18)): 15 прогонов на том же USB-накопителе (`after-b1`, `before-b2`, `after-b2` × 5), все `verify ok=true`; поколение после барьера 2 сохранено 5 из 5. Все отключения позже 2 с после точки (3,3–22,6 с) — слабое свидетельство; внутренний SSD не испытывался | 15 прогонов в трёх точках отключения (`idle`, `block0`, `rootcopy`), по 5 на стадию; все `verify ok=true`; см. [S9_39_POWER_LOSS_PLAN.md §7](../testing/S9_39_POWER_LOSS_PLAN.md). Пределы (итоговое ревью S9-43, для прогонов §7; точки между барьерами покрыты в §8.7): во всех стадиях отключение ручное после простоя накопителя, поэтому ни один прогон не доказывает, что `F_FULLFSYNC` сработал как барьер; `rootcopy` по построению почти не добавляет покрытия к `block0`; носитель — внешний USB, исключённый из целевой комбинации (внутренний SSD, STORAGE_FORMAT §3) |
| S9-42 | принята (решение владельца 2026-11-18) | Скрипт `tools/stage9/run_checks.sh` и [STAGE_9_REPRO.md](../testing/STAGE_9_REPRO.md). Обновлено 2026-11-18: добавлены case `od_host_persistent_nvme` (S9-04) и `s9_40_bench_once` (S9-40), статичные строки приведены к факту; добавлена строка S9-38, статусы приведены к словарю; последний запуск `20261118-s9-38c` (срез S9-38 уровень 1): 12 passed, 0 failed, 1 not_run, 7 blocked; нестабильность теста этапа 5 [ISSUE-518](../KNOWN_ISSUES.md#issue-518-s5-persistent-nvme-reopen-test-flaky-in-parallel) закрыта общим `process_spawn_guard`. Hardware/Windows/review case остаются `blocked`/`not_run`; зависит от S9-39 (принята в объёме USB-накопителя) и S9-41 (ревью Opus high 2026-11-18) |
| S9-44 | задача (запись приёмки) принята владельцем 2026-11-18; этап НЕ принят | §7 и §10 фиксируют решение и оставшиеся ограничения; итоговое ревью S9-43 storage-scope засчитано, S9-22 закрыта в объёме v1 (решения владельца 2026-11-18) |
| S9-43 | выполнено в storage-scope; ревью засчитано (решение владельца 2026-11-18) | Итоговое ревью storage-части проведено 2026-11-18 Claude Opus (high) по правилам AGENTS.md «Правила для Claude» (§10): код готов к приёму в заявленном объёме, дефектов кода уровня blocker/major нет; major в спецификации и minor исправлены. Отклонение от плана (§3 п. 4, S9-43: ревьюер `gpt-6.1-sol`) засчитано владельцем 2026-11-18. Полная VM (S9-23…38) не ревьюирована — её нет; не закрыты S9-38 (VM-часть), S9-39 (целевой носитель), S9-23…37 |

Для S9-07…S9-12 в таблице строк нет: они описаны в §2. Дополнительно: namespace durability вне объёма v1 (S9-06 закрыта в суженном объёме, ISSUE-055 — предел) и влияет на S9-10, S9-11, S9-29 (публикация root/manifest).

## 4. Gates (план, раздел 8)

| Gate | Статус | Доказательство или недостаток |
|---|---|---|
| G1 — independent storage (S9-03…12) | `partial` | Есть: I/O contract, writer lock, RAW, file-data flush, fault backend с независимым oracle (S9-07), CoW-протокол v1 (S9-08), строгий parser (S9-09), open и выбор корня (S9-10), write transaction (S9-11), recovery tests (S9-12) на модели отказов. Формат CoW v1 принят как нормативный 2026-11-06, задачи S9-03, S9-05, S9-07…S9-12 приняты владельцем 2026-11-18 (S9-04 частично, S9-06 в суженном объёме); gate G1 не объявлен выполненным; S9-40 частичный (checkpoint/restore). Нет: namespace durability, интеграции с VMHost (S9-04), power-loss на целевом носителе; ветка `feat/s9-10-cow-open` с отдельными черновиками не объединена. G1 остаётся `partial` |
| G2 — snapshots/tools (S9-14…22) | `partial` | Есть: immutable snapshots и дочерние цепочки (S9-14, `7188582`), branch и rollback как новая ветка (S9-15, `33440be`), проверка цепочки предков и leases до записи (S9-16, `33440be`), удаление snapshot и offline compaction (S9-17, `82f72e8`), capacity/zero/discard (S9-18, `1590c9e`), read-only inspect/check CLI (S9-19, `1590c9e`), RAW export/import и salvage в новый target (S9-20, S9-21, `fd5ebcf`; CLI `import` 2026-11-18; ревью субагентом 2026-11-06, срез 2026-11-18 — Opus high). Формат CoW v1 принят 2026-11-06 (слой ODSNAP — нет). Gate требует S9-14…22 целиком; S9-22 закрыта в объёме v1 (решение владельца 2026-11-18): таблица совместимости (STORAGE_FORMAT §9.7), путь RAW → v1 через `import`; миграции между версиями CoW нет (второй версии нет). Все задачи S9-14…S9-22 приняты владельцем 2026-11-18; сам gate G2 владельцем не объявлен выполненным. Пределы: GC в живом файле, hole punching, repair-подкоманды нет; нет durable rename и атомарного активного указателя (ISSUE-055); resolver цепочки только в офлайн-CLI image-tool (`PathResolver`, `1590c9e`), VMHost цепочку не открывает |
| G3 — coherent full VM (S9-13, 23…31) | `not_met` | Не начато; блокированы ISSUE-056 и отсутствующие компоненты |
| G4 — identity/integration (S9-32…37) | `not_met` | Не начато |
| G5 — failure qualification (S9-38/39/41) | `partial` | Есть deterministic fault model и seeded-прогоны для RAW и CoW (S9-07/08/10/11/12), cut-point sweeps, repeated crashed recovery на модели, soak на модели (S9-41, `e57646c`: 500 циклов на seed, 3 seeds по умолчанию; длинный прогон 8 seeds × 12000 циклов без расхождений 2026-11-18; ревью Opus high), unit-тесты sticky reconcile и kill процесса (S9-04). Матрица S9-38 уровней 0 и 1 для storage (2026-11-18, STORAGE_RECOVERY §4.1–4.2). Нет: VM-части S9-38, power-loss на целевом носителе (внутренний SSD) и доказательства барьерной семантики (S9-39); soak не заменяет испытаний реального носителя |
| G6 — reproducibility (S9-40…44) | `not_met` | S9-40 частичный (storage-замеры и бенчмарк в репозитории; checkpoint/restore заблокированы S9-29…31); S9-38 закрыта в объёме storage, VM-часть `blocked`; S9-42 и S9-44 приняты 2026-11-18 (G6 остаётся `not_met` из-за S9-40 и VM-частей); итоговое ревью S9-43 storage-scope проведено и засчитано (§10), ревью полной VM невозможно |

## 5. Открытые блокеры и записи

- [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed): гарантия durability macOS/APFS не подтверждена. Предел v1: durable-гарантия только для данных файла (`F_FULLFSYNC`, S9-06 закрыта в суженном объёме); namespace durability и проверенного источника нет; уровень 1 выполнен (STORAGE_RECOVERY §4.2), уровни 2–3 нет.
- [ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner): у архитектурного CPU state нет единого versioned owner; блокирует S9-24 и full-VM checkpoint.
- Формат CoW v1 принят 2026-11-06, задачи S9-08…S9-12 приняты 2026-11-18; S9-40 частичный (checkpoint/restore заблокированы S9-29…31); слой ODSNAP не нормативен. Устройства хранения гостя нет (S9-13).
- Цепочку snapshots открывает только офлайн-CLI (`PathResolver` с `ParentLease`, `1590c9e`); VMHost её не открывает; нет атомарного активного указателя ветки (нужен durable rename, ISSUE-055); CRC блоков предков проверяется лениво.
- Ветки разошлись: `feat/s9-stage` и `feat/s9-10-cow-open` содержат разные коммиты S9-10…S9-12 (общий предок `17c9ce6`). Требуется синхронизация перед приёмкой G1; до неё кросс-ветка состояние не считается единым.
- [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable): закрыт (`01ef35d`); остаётся как справка по решению (create заполняет оба слота).
- Нет устройства хранения гостя (S9-13), UEFI, vTPM (S9-26/27).
- Нет разрешения на вариант B S9-39 — отключение питания Mac для внутреннего SSD.
- S9-04: read lease (`31b8c2a`), busy-политика export/repair и `ParentLease` (`bc7b675`) реализованы; публичный путь `attach_persistent_nvme` проверен двумя процессами (2026-11-18); parent в цепочке проверяется (S9-16, `ParentLease`). Нет интеграции с VMHost lifecycle (VMHost не подключает гостю диски); non-advisory lock на сетевых ФС не предоставляется (вне гарантий); locks advisory.
- Пункты 1–3 расхождений документов с кодом (STORAGE_FORMAT.md шапка, §5, §7; ISSUE-055) закрыты; прочие пункты архитектурного ревью S9-00…02 применены в документах (см. S9-00…02 выше). Остаются: [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed) (namespace, power-loss, ENOTSUP/EINVAL), лимит logical size RAW. §9 STORAGE_FORMAT.md нормативен с 2026-11-06 (слой ODSNAP — нет, §9.8).

## 6. Тесты

Команды и результаты: `feat/s9-stage` на `e57646c` (2026-11-05) и `feat/s9-10-cow-open` на `432d07e` (проверено 2026-11-05 в `/private/tmp/open_desktop-s9-10`).

| Команда | Итог |
|---|---|
| `cargo test -p od-storage` | 205 passed, 1 ignored, 0 failed на `b5f75d3` (201 на `e57646c`; 175 на `1590c9e`; 156 на `82f72e8`; 143 на `33440be`; 36 до S9-07/08; 45 на `a7874b8`; 59 на `000c1da`; 90 на `88b9f42`; 113 на `7d23bf9`) |
| `cargo test -p od-storage` на `feat/s9-10-cow-open` (`432d07e`) | 153 passed, 0 failed: cow_parse 17, cow_proto 1 + cow_proto_tests 17, cow_tx 13, cow_recovery 14, cow_chain 21, cow_branch 10, cow_lease 15, io 23, raw 13, testing 9 |
| `cargo test -p od-host --lib storage_io` | 30 passed, 0 failed (19 до `31b8c2a`) |
| `cargo test -p od-host --lib storage_policy` | 20 passed, 0 failed (17 на `1590c9e`; 16 на `82f72e8`; 9 на `bc7b675`; 15 после lease-тестов S9-14…16) |
| `cargo test -p od-host --lib raw_file_tests` | 9 passed, 0 failed |
| `cargo test` в `tools/image-tool` | 16 passed, 0 failed (7 на `1590c9e`; `tests/cli.rs`) |
| `cargo test -p od-host` | 253 passed, **21 failed** (`s3_20*` и `native_register_v1_corpus::reduce_saved_native_register_v1_case`) |

21 падение в полном `cargo test -p od-host` существовало до этапа 9 (по сведениям из задания, проверено на чистом HEAD до S9); в этой сессии падения воспроизведены на текущем состоянии, повторная проверка на чистом HEAD не выполнялась. Они не относятся к коду S9-03…06 и не закрыты. Тесты S9 на этих падениях не основаны.

Что тесты не доказывают: power-loss, namespace durability, поведение на других volumes и в реальных VMHost; результаты на `FaultBackend` не заменяют испытаний реального носителя.

## 7. Приёмка (S9-44)

**Решение: этап 9 НЕ принят.** Gates G1–G6 не объявляются выполненными: G1, G2, G5 `partial`; G3, G4, G6 `not_met`. Приняты решениями владельца 2026-11-18 26 задач: S9-22 (в объёме CoW v1), S9-06 (в суженном объёме v1: durable только данные файла), S9-38 (в объёме storage), S9-39 (в объёме USB-накопителя), а также S9-00…S9-03, S9-05, S9-07…S9-12, S9-14…S9-21, S9-41, S9-42, S9-44 в реализованном объёме; не приняты S9-04, S9-40 (частично), S9-43 (ревью засчитано), S9-13, S9-23…S9-37 (`blocked`). Итоговое ревью S9-43 в storage-scope проведено 2026-11-18 (Claude Opus high, §10); владелец засчитал его вместо названного в плане ревьюера. S9-22 закрыта в объёме v1 (решение владельца 2026-11-18).

Воспроизводимый набор: [STAGE_9_REPRO.md](../testing/STAGE_9_REPRO.md), запуск `tools/stage9/run_checks.sh`. Последний запуск — в STAGE_9_REPRO.md §4 (2026-11-18); первый (run-id `20261105-final`, HEAD `7c68b98`): 9 passed, 0 failed, 3 not_run, 4 blocked. Результаты лежат в игнорируемом `work/stage9/<run-id>/` и не версионируются; в Git — только скрипт и документ.

### Реальные гарантии (доказаны тестами на модели отказов и локальных файлах)

- Единый storage I/O contract, single-writer lock, read leases и busy-политика; RAW backend с flush данных файла (`F_FULLFSYNC` на macOS).
- Детерминированный `FaultBackend` с независимым oracle: на cut-point sweeps и seeded-прогонах CoW-протокол v1 (два root slot, копия корня, барьеры) не теряет durable watermark и не открывает старую валидную root как свежую; create/open, write transaction и recovery работают на этой модели.
- Строгий read-only parser (fuzz 80000 разборов без паник), неизменяемые snapshots и цепочки, branch и rollback как новая ветка, проверка parent identities до записи, удаление snapshot и offline compaction, capacity/zero/discard с ENOSPC-инъекцией, read-only `inspect`/`check` CLI.
- RAW export/import и salvage в новый target (ревью субагентом 2026-11-06; правки 2026-11-18 — Opus high).
- Ограниченный soak на модели (S9-41): 8 seeds × 12000 циклов без расхождений (выполнено и записано 2026-11-18, оракул сверяет и `next_snapshot_id`).
- Команды набора §3 STAGE_9_REPRO.md проходят на указанном HEAD (последний запуск — см. STAGE_9_REPRO.md §4); baseline 45 ERROR у `check_layout.py` известен и не растёт.
- Единственный writer между процессами на публичном пути подключения диска (S9-04, SIGKILL); CLI `import` RAW → CoW v1 (S9-20); воспроизводимые storage-замеры S9-40.

### Не доказано

- Power loss и durability namespace на macOS/APFS ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)): 30 прогонов S9-39 (§7 и §8.7, включая точки между барьерами) выполнены на внешнем USB-накопителе вне целевой комбинации; барьерная семантика `F_FULLFSYNC` не доказана (все отключения позже 2 с); S9-39 принята владельцем в этом объёме; kill процесса и fault-инъекция стенд не заменяют.
- Замеры S9-40 для checkpoint/restore (заблокированы S9-29…31) и на реальных размерах диска; migration между версиями CoW (S9-22 закрыта в объёме v1: второй версии нет). Формат CoW v1 принят 2026-11-06; RAW против CoW по throughput, чтению, WA, open и памяти измерено (S9-40).
- Атомарный активный указатель ветки, resolver цепочки в VMHost (в офлайн-CLI есть), GC в живом файле, интеграция с VMHost lifecycle; ветки `feat/s9-stage` и `feat/s9-10-cow-open` не синхронизированы.
- Coherent full-VM checkpoint, CPU state ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)), устройство хранения гостя (S9-13), UEFI, vTPM, Windows, Linux Mini, clone/identity, lifecycle и конкурирующие операции (S9-23…S9-37): не начаты или `blocked`.
- VM-часть матрицы S9-38; итоговое ревью S9-43 проведено для storage-части и засчитано (§10), ревью полной VM невозможно; S9-20, S9-21, S9-41 — ревью субагентом 2026-11-06 и Opus high 2026-11-18, приняты владельцем 2026-11-18.
- Disk snapshot не является backup всех данных VM, и application consistency или filesystem freeze не заявляются.

### Следующие шаги до приёмки

Решения владельца 2026-11-18: ревью S9-43 Claude Opus high засчитано; S9-22 закрыта в объёме v1; S9-06 — в суженном объёме v1; S9-38 — в объёме storage; S9-39 — в объёме USB-накопителя. Остаётся решение о приёме остальных задач storage-части по §10. Работы: испытание целевого носителя (внутренний SSD) и доказательство барьерной семантики, источник для ISSUE-055; VM-часть S9-38; компоненты этапов 4–8 для G3/G4 (S9-13, S9-23…37). Только после этого пересматривать §4 и этот раздел.

## 8. Ревью 2026-11-06 (субагент, только чтение)

Объект: HEAD `232049b`, S9-20/21 (`fd5ebcf`), S9-41 (`e57646c`), сквозная проверка S9-43. Ревьюер — субагент на Opus, не требуемый внешний ревьюер; его вывод записан как внутренний независимый проход. Запуски: `cargo test -p od-storage` 205 passed, 1 ignored; `cargo test -p od-image-tool` 16 passed; `od-host` storage_policy 20, storage_io 30, raw_file_tests 9 passed. Полный `od-host` даёт 21 падение в JIT-тестах (`s3_20*`, `native_register_v1_corpus`), они задокументированы в [STAGE_9_REPRO.md](../testing/STAGE_9_REPRO.md) и к хранилищу не относятся.

Блокеров и мажоров нет. Исправлено после ревью: пункты 1, 3, 5, 6 и 2 и 4 (решения Opus high). Открыты: 7–8 и остаток пункта 2 (удаление `partial` при успехе, см. ниже).

1. [исправлено; регрессионный тест `unordered_roots_that_agree_are_partial_and_never_complete` добавлен, см. §9] `cow_salvage.rs:847-849` (ветка ambiguous, :723-735): два неупорядоченных валидных корня без подтверждённого порядка и совпадающими байтами выбранного состояния дают `recoverability=Complete`, `possible_lost_commit=false`, код выхода 0. По документации модуля (:44-46) Complete требует доказанного newest root. Snapshots в этом режиме не сравниваются (:780). Нужна правка: не Complete без доказательства либо явный отдельный класс.
2. [исправлено] `storage_policy.rs`: при несовпадении identity `target` больше не удаляется; возвращается `PublishError::TargetUnverified` (код `target_unverified`, `target_left=true`, код возврата 5). Регрессионные тесты: подмена `target` после link, удаление `target` после link. Остаток: удаление `partial` при успешной публикации остаётся path-операцией (гонка подмены `partial` между проверкой и `remove_file`); решение — принять и документировать: записано как принятый предел [ISSUE-517](../KNOWN_ISSUES.md#issue-517-s9-20-partial-removal-is-a-path-operation) (2026-11-18).
3. [исправлено: `symlink_metadata` для partial и target] Гипотеза, не воспроизведена: `storage_policy.rs:190-193` берёт identity через `std::fs::metadata`, который следует symlink; нужен `symlink_metadata`.
4. [исправлено] Гипотеза (окно после снятия flock): `.partial` теперь держится под `LOCK_EX` до возврата `publish_no_replace` (writer передаётся по ссылке), создаётся с правами `0600`. Не закрывается: процесс того же пользователя, который не соблюдает `flock`, может писать в файл (advisory lock); байты после публикации не перечитываются.
5. [исправлено 2026-11-18: `salvage_to_copy` до первой записи отказывает цели без записи/resize (`target_unsuitable`) или без file durability (`durability_unavailable`), как export; тест `cow_salvage_tests::a_target_that_cannot_be_durable_is_refused_before_any_write` (до правки цель получала 3 операции и оставалась building)] Гипотеза, Linux не запускался: `cow_salvage.rs:902-913` (на 2026-11-18) сводит всё, кроме ENOSPC, к `target_io_error`; при отсутствии file durability `UnsupportedDurability` даёт оставленный building-файл, тогда как export отказывает заранее.
6. [исправлено 2026-11-18: `Soak::resolve` после каждого восстановления и `check_cycle` после каждого цикла читают `next_snapshot_id` опубликованного заголовка через `inspect_storage` (отдельное read-only представление, тот же корень) и сверяют с оракулом; мутация ожидания ловится; ревью Opus high; длинный прогон — §2, S9-41] `cow_soak_tests.rs:308-311, 756-771`: `next_snapshot_id` файла не сравнивается с оракулом; заголовок модуля (:14-15) обещает больше, чем проверяется.
7. Устаревшие утверждения в отчёте исправлены в этом срезе: export/repair S9-20/21 реализованы (`fd5ebcf`), S9_40 в коммите `e2e6892`, throughput измерен (`232049b`).
8. ODSNAP (snapshot-слой: header, kind, бит building, `MAX_SNAPSHOTS = 8`, `MAX_DISK_BLOCKS = 4096`) не описан в [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md) §9 и не входит в принятие формата; на него опираются S9-20/21.

Не проверено: поведение на Linux; реальный power loss и durability имени в каталоге (ISSUE-055); длинные soak через `OD_SOAK_*` (выполнены 2026-11-18, см. S9-41 в §2); гипотезы 3–4 не воспроизводились (запрещено писать в файлы).

### Ревью 19c1ac6 (Opus, high, 2026-11-06)

Найдено и исправлено в документах: §9 `STORAGE_FORMAT.md` противоречил шапке (прототип «не нормативен»), не описывал классы слотов и правило N/N−1, поколения 0 и `u64::MAX`, выравнивание копии корня, точки отмены и poisoning, а также смешивал нормативную часть с ODSNAP (теперь §9.8). Устаревшие «гипотезы» в шапке и таблицах спецификации и в комментариях `cow_parse.rs`/`cow_proto.rs` заменены на «CoW v1». Пункты 2 и 4 закрыты по решениям Opus high (см. §8). Нового регрессионного теста на `unordered_roots` при всех Valid нет.

## 9. Актуализация 2026-11-06

Раздел сверён с HEAD и имеет приоритет над устаревшими формулировками выше.

- **Формат CoW v1** принят владельцем проекта как нормативный 2026-11-06 ([STORAGE_FORMAT.md §9](../specs/STORAGE_FORMAT.md)). Слой ODSNAP в принятие не входит. S9-40 частично: commit-latency, throughput, open/roll-forward и память измерены (обновлено 2026-11-18: и цепочки, export, compaction воспроизводимым бенчмарком); checkpoint/restore — нет.
- **S9-39**: процедура принята владельцем 2026-11-06 ([S9_39_POWER_LOSS_PLAN.md](../testing/S9_39_POWER_LOSS_PLAN.md)). Выполнено 15 прогонов: `idle`, `block0`, `rootcopy`, по 5 на стадию (сокращено с 20 по решению пользователя). Все прошли `verify`, подтверждённые коммиты сохранились. Не покрыто: точки между барьерами 1 и 2 (драйвер их не останавливает), задержка между подтверждением и отключением в `idle` не фиксировалась. Стадии `block0` и `rootcopy` лежат до барьера 1, поэтому барьерную семантику не проверяют. Приём результатов не выполнен.
- **Модель накопителя** (`disk6`, том `128`, `disk7s1`): USB-продукт `Disk 20`, производитель `USB`, версия устройства `8.00` (`bcdDevice`); тип флеш-памяти и контроллер не определены (`SMART: Not Supported`).
- **Ревью S9-43** (fable, medium, только чтение): блокеров и мажоров нет, формально задача не закрыта без `gpt-6.1-sol`. Миноры: этот раздел и пределы S9-39 (выше). Итоговое ревью 2026-11-18 — §10.
- **Регрессионный тест** `unordered_roots_that_agree_are_partial_and_never_complete` добавлен (`cow_salvage_tests.rs`): два корня с одинаковым состоянием и поколениями 3 и 6 дают `Partial` с причиной `unordered_roots` и не создают цель.
- **Миноры, которые остаются открытыми**: удаление `partial` после успешной публикации (path-операция, см. §8); ODSNAP не нормативен (§9.8 STORAGE_FORMAT.md).
- **Этап 9 не принят.** Блокеры: S9-43 требует `gpt-6.1-sol` (снято решением владельца 2026-11-18, §10); приём S9-39 не выполнен; S9-38, S9-42 и остальные задачи этапа не закрыты (на 2026-11-06; S9-39 принята в объёме USB-накопителя 2026-11-18; S9-38 закрыта в объёме storage 2026-11-18, §10).

## 10. Итоговое ревью S9-43 и актуализация 2026-11-18

Ветка `codex`. Срезы этого дня, каждый с независимым ревью Claude Opus (high) до коммита:
- `1e4b96b` — ISSUE-086;
- `4c2b8c9` — S9-21;
- `88ea1ca` — S9-41;
- `cc0d577` — S9-04;
- `c423e0c` — S9-20/S9-22;
- `d27de16` — S9-40;
- `3719cc9` — актуализация документов;
- `cdf8361` — S9-42.

**Итоговое ревью S9-43 (Claude Opus, high, только чтение, HEAD `cdf8361`).** Объём: storage-часть — S9-00…22, процедура и результаты S9-39, S9-40, S9-41, S9-42. Прогоны ревьюера:
- `od-storage` — 210 passed, 1 ignored;
- `od-image-tool` — 21 passed;
- `od-host` storage_io и storage_policy — 80 passed;
- `raw_file_tests` — 9 passed;
- `persistent_nvme` — 19 passed.

- **Вердикт.** Код хранилища готов к приёму в заявленном объёме. Контракты плана §4.1–4.2 проверены по коду:
  - ack только после барьера 2, ack без барьера доступен лишь под `cfg(test)`;
  - нет отката старше подтверждённого коммита: writable open с необъяснённым повреждением отказывает, roll-forward только с полной проверкой;
  - неопределённый исход отравляет сессию и переводит хранилище в sticky reconcile, adapter NVMe становится терминальным, повторов нет;
  - bounds проверяются до выделения памяти;
  - export, salvage и import пишут только в новый target и до записи отказывают цели без durability;
  - опубликованное состояние неизменяемо.

  Дефектов кода уровня blocker или major нет.
- **Исправлено по ревью (этот срез).**
  - Major: в нормативном §9.6 STORAGE_FORMAT не было правила roll-forward и смысла `E`. Дописаны условия принятия копии корня, полная проверка, отказ без отката, поведение writable и read-only open, а также `possible_lost_commit`/`DamagedOtherSlot` и хвост за `E` (§9.1). Это описание поведения v1, а не изменение формата.
  - Minor:
    - устаревшие строки отчёта (§3 S9-39 и S9-43, §5, §7, шапка);
    - `STORAGE_RECOVERY.md` §3.1 и §5;
    - комментарии `cow_tx.rs`, `cow_parse.rs`, `raw.rs`.
  - Причина ISSUE-518 уже была описана у `process_spawn_guard`. Тесты `persistent_nvme` с файлами теперь берут этот guard, ISSUE-518 закрыта (40 параллельных прогонов без падений).
- **Пределы S9-39, уточнённые ревью.**
  - Ни один из 15 прогонов не доказывает барьерную семантику `F_FULLFSYNC`: отключение ручное после простоя накопителя.
  - `rootcopy` почти не добавляет покрытия к `block0`.
  - Носитель — внешний USB вне целевой комбинации.
- **Не покрыто ревью.**
  - `cow_snapshot.rs` (compaction, rootset), `cow_salvage.rs` и `cow_inspect.rs` прочитаны выборочно.
  - Цифры S9-40 не перепроверялись.
  - Тестовые файлы по содержанию не ревьюировались.
  - Linux и реальная потеря питания не проверялись.
- **Отклонение.** План (§3 п. 4 и S9-43) называет ревьюера `gpt-6.1-sol`. Ревью выполнено Claude Opus high по AGENTS.md («Правила для Claude») и засчитано владельцем 2026-11-18.
- **Известное, не исправлялось.** В `KNOWN_ISSUES.md` повторяются номера ISSUE-055 и ISSUE-056 (разные якоря, `s9-…` и без); перенумерация сломала бы ссылки других этапов. Ссылаться на них по якорю.

**S9-44 (приёмка).** Решение прежнее: **этап 9 не принят** (§7). Storage-часть реализована и прошла итоговое ревью в своём объёме. Решения владельца 2026-11-18: ревью S9-43 засчитано, S9-22 закрыта в объёме v1. Полная VM (G3, G4) и VM-часть матрицы S9-38 остаются открытыми; S9-39 принята в объёме USB-накопителя (решение владельца 2026-11-18).

**Решения владельца 2026-11-18 (дополнение).** S9-38 закрыта в объёме storage: уровни 0 и 1, VM-часть остаётся строкой `S9-38_vm_matrix` (`blocked`). S9-06 закрыта в суженном объёме v1: durable-гарантия даётся только для данных файла через `F_FULLFSYNC` на macOS (порядок вызовов commit проверен на реальном файле, kill процесса — уровень 1). Создание, переименование и публикация файлов — завершение без durable-гарантии; [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed) остаётся открытой как предел v1. Что `F_FULLFSYNC` даёт на носителе, испытаниями не доказано (S9-39). По S9-39 решено подготовить новый план испытаний с точками между барьерами.

**Решение владельца 2026-11-18 (S9-39).** S9-39 принята в объёме USB-накопителя варианта A: 30 прогонов (§7 и §8.7), барьерная семантика `F_FULLFSYNC` не доказана (все отключения позже 2 с), внутренний SSD целевой комбинации не испытан. Результаты — [S9_39_POWER_LOSS_PLAN.md §8.7](../testing/S9_39_POWER_LOSS_PLAN.md#87-результаты-2026-11-18). Утверждение о power-loss durability для целевой комбинации (внутренний SSD) не делается.

**Решение владельца 2026-11-18 (приёмка storage-части).** Приняты в реализованном объёме по итогам ревью S9-43: S9-00…S9-03, S9-05, S9-07…S9-12, S9-14…S9-21, S9-41, S9-42 и S9-44 (как запись приёмки). Ограничения каждой задачи остаются в её разделе выше. S9-04 (нет интеграции с VMHost) и S9-40 (нет замеров checkpoint/restore) остаются частичными. **Этап 9 не принят:** G3/G4 (полная VM, S9-13, S9-23…S9-37) не выполнены.
