# Методика испытаний storage recovery

Статус: **S9-00 принято с оговорками по архитектурному ревью**. Документ описывает уровни испытаний и правила evidence. Фактические статусы на 2026-11-05 — в §3.1: уровень 0 выполнен на модели для RAW (S9-07) и прототипа CoW (S9-08…12); уровень 1 выполнен частично (lock и SIGKILL, S9-04); уровень 2 `not_run`; уровень 3 `blocked`/`not_run` до подготовки стенда и разрешения пользователя.

Контракт гарантий и модель отказов — [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md) (§6 уровни подтверждения, §8 модель отказов). Операции VM — [VM_STATE.md](../specs/VM_STATE.md). План — [этап 9](../plans/STAGE_9.md), разделы 5 и 8. Стенд x86 — [STAND.md](STAND.md).

## 1. Общие правила

- **Независимый oracle.** Ожидаемое состояние считает отдельная простая логическая модель (последовательность операций, durable watermark, referenced snapshots). Reader и writer, использующие один parser, не проверяют друг друга.
- **Журнал acknowledgement вне faulted target.** Для каждого `durable_ack` фиксируются sequence number, operation ID и generation до продолжения работы; журнал хранится в процессе/на носителе, не затрагиваемом отказом. Для уровней 2–3 — на другой машине или носителе вне persistence domain.
- **Критерий PASS.** После recovery: (а) всё с `durable_ack` присутствует, с учётом законных последующих overwrite/discard; (б) unacknowledged операции дают только разрешённые целые исходы; (в) referenced snapshots неизменны; (г) corruption вне модели обнаружена явно. Открытие старого валидного root с потерей acknowledged write — **FAIL**, даже если image «валиден».
- **Неопределённый исход.** Операция с `uncertain` проверяется через reconcile по operation ID/generation; тест, выполняющий blind retry, некорректен.
- **Статусы результатов:** `passed`, `failed`, `not_run`, `blocked`. Каждый case имеет environment fingerprint (host OS/build, filesystem, volume, media, commit, build flags), seed, operation trace и evidence reference. Большие images/logs — в ignored `work/stage9/<run-id>/`; в Git — только малые собственные fixtures и отчёты без секретов.
- **Отказ на одном уровне не закрывается evidence другого уровня.**

## 2. Уровни evidence

### Уровень 0 — deterministic fault model

| | |
|---|---|
| Что моделирует | Fault backend (S9-07) хранит volatile и durable bytes и namespace раздельно; на заданном cut теряет/переупорядочивает/рвёт unsynced writes, возвращает short writes и ошибки sync/rename/allocate/ENOSPC согласно [модели отказов](../specs/STORAGE_FORMAT.md#8-модель-отказов-crash-model) |
| Доказывает | Корректность протокола и recovery **относительно модели**: при каждом перечисленном cut recovery не теряет durable watermark; порядок data → metadata → publication → ack соблюдён в коде; ошибки не превращаются в успех |
| Не доказывает | Что реальный host API/filesystem/накопитель ведут себя как модель; что модель полна |
| Обязательная самопроверка | Специально сломанный flush order или ранний ack должен давать FAIL; иначе harness не принимается |
| Где | Unit tests `od-storage`, orchestration `tests/system/`; production не зависит от test backend |
| Воспроизводимость | Seed и полный operation trace сохраняются; любой mismatch воспроизводится из них |

### Уровень 1 — аварийное завершение процесса

| | |
|---|---|
| Что моделирует | `SIGKILL` одноразового writer-процесса (или VMHost) на разных стадиях write/flush/snapshot/export |
| Доказывает | Recovery после смерти процесса в любой точке: reopen, обработка unfinished candidate, освобождение/повторный захват writer ownership, отсутствие повторного mutating действия после restart (S9-36) |
| Не доказывает | Durability: host page cache и caches накопителя остаются живыми, поэтому completed-but-unsynced данные обычно переживают kill. **Process kill не является power-loss испытанием** и не может так называться |
| Где | Локальный Mac на одноразовых файлах; Linux Mini — только для переносимости bytes |

### Уровень 2 — авария/перезапуск host

| | |
|---|---|
| Что моделирует | Потерю host OS state (принудительный перезапуск/panic host) без отключения питания накопителя |
| Доказывает | Что данные с `durable_ack` переживают потерю kernel caches в выбранной комбинации; поведение namespace (create/rename) после перезапуска |
| Не доказывает | Сохранность при потере питания: накопитель может оставаться под питанием и сбросить свой volatile cache. Результат относится только к испытанной комбинации OS/filesystem/media |
| Безопасность | Только на выделенном стенде после явного разрешения пользователя на конкретный перезапуск. Рабочий Mac и Mini автоматически не перезапускаются |

### Уровень 3 — реальная потеря питания

| | |
|---|---|
| Что моделирует | Отключение питания всего persistence domain выбранной комбинации (host + накопитель) на выбранных стадиях |
| Доказывает | Заявленную durability **для конкретной** модели Mac/накопителя/контроллера/firmware/filesystem при указанном числе прогонов |
| Не доказывает | Свойства других накопителей, внешних дисков, других версий OS; абсолютное отсутствие ошибок (ограниченное число прогонов) |
| Подготовка (S9-39) | Схема питания и storage stack, одноразовый volume, перечень затрагиваемых носителей, recovery процедура, внешний журнал ack. Затем — конкретное разрешение пользователя |
| Запреты | Не выключать рабочий Mac/Mini, не форматировать системный диск, не отключать чужой накопитель автоматически. Отключение внешнего накопителя проверяет только описанную комбинацию, а не внутренний SSD |
| Статус | `blocked`/`not_run`, пока нет безопасного стенда и разрешения; остальные работы продолжаются |

## 3. Соответствие уровней и задач

| Задача | Уровень 0 | Уровень 1 | Уровень 2 | Уровень 3 |
|---|---|---|---|---|
| S9-07 RAW write→flush→crash→reopen | обязателен | желателен | — | — |
| S9-06 durability path | обязателен | обязателен | требуется для claim | требуется для power-loss claim |
| S9-10…12 CoW create/transaction/recovery | обязателен на каждом cut | обязателен | — | — |
| S9-36 recovery после аварии VMHost | — | обязателен | — | — |
| S9-38 fault/corruption matrix | обязателен | обязателен | — | — |
| S9-39 power loss | — | — | по процедуре | обязателен |
| S9-41 repeated recovery/soak | обязателен | обязателен | — | — |

### 3.1. Фактические статусы (2026-11-05, коммит `7d23bf9`; строки S9-39 и S9-41 обновлены 2026-11-18)

| Задача | Уровень 0 | Уровень 1 | Уровень 2 | Уровень 3 |
|---|---|---|---|---|
| S9-07 RAW write→flush→crash→reopen | `passed` на модели: [`testing/fault_tests.rs`](../../crates/od-storage/src/testing/fault_tests.rs), самопроверка oracle (`oracle_detects_ack_without_durability`, `seeded_runs_detect_broken_flush_and_ack`); namespace в fault backend не моделируется | `not_run` | `not_run` | `blocked`/`not_run` |
| S9-04 writer lock и read lease | — | `partial`: `writer_in_another_process_blocks_us_and_crash_releases_it` и `lease_in_another_process_blocks_writer_allows_readers_and_sigkill_releases_it` (SIGKILL процесса, lock освобождён) в [`storage_io.rs`](../../crates/od-host/src/storage_io.rs); нет kill writer во время write/flush и в VMHost | `not_run` | `blocked`/`not_run` |
| S9-06 durability path | `partial`: sticky `reconcile_required` на memory-backend ([`io_tests.rs`](../../crates/od-storage/src/io_tests.rs)); реальный `F_FULLFSYNC` вызывается в тестах host backend, но без журнала порядка операций; namespace не реализован | `not_run` | `not_run` | `blocked`/`not_run` |
| S9-08, S9-10…12 CoW v1 | `passed` на модели: cut-point sweeps ([`cow_proto_tests.rs`](../../crates/od-storage/src/cow_proto_tests.rs), [`cow_open_tests.rs`](../../crates/od-storage/src/cow_open_tests.rs), [`cow_tx_tests.rs`](../../crates/od-storage/src/cow_tx_tests.rs), [`cow_recovery_tests.rs`](../../crates/od-storage/src/cow_recovery_tests.rs)); ISSUE-057 закрыт (`01ef35d`, sweeps по каждому cut create/commit); формат CoW v1 принят как нормативный 2026-11-06 | `not_run` | `not_run` | `blocked`/`not_run` |
| S9-09 parser corruption | fuzz 80000 мутаций (soundness) в [`cow_parse_tests.rs`](../../crates/od-storage/src/cow_parse_tests.rs) | — | — | — |
| S9-41 soak | `passed` на модели: 8 seeds × 12000 циклов (2026-11-18, лог в `docs/reports/data/stage9/`) | — | — | — |
| S9-39 power loss | — | — | — | выполнено частично (2026-11-06): 15 прогонов `verify ok` на внешнем USB-накопителе вне целевой комбинации; точки между барьерами не покрыты, барьерная семантика не доказана; приём не выполнен ([S9_39_POWER_LOSS_PLAN.md](S9_39_POWER_LOSS_PLAN.md)) |
| S9-36, S9-38 | `not_run` | `not_run` | `not_run` | `blocked`/`not_run` |

Все результаты уровня 0 относятся к модели и не доказывают поведение реального host API, filesystem и накопителя.

Claim «durable после power loss» допустим только для комбинации с `passed` на уровне 3. До этого документация говорит «durable в модели уровня 0/1/2», явно перечисляя уровень.

## 4. Повреждения (corruption)

Повреждения вносятся отдельно от crash cuts: усечение, порча lengths/root/map/payload, wrong version/identity, duplicate/conflicting entries, циклы и превышение depth. Ожидаемый исход — явная ошибка с местом и типом либо salvage в новый target с отчётом о valid/unknown/lost ranges. Bounded fuzzing parser'а дополняет, но не заменяет доказательство данных: «ноль crashes parser» не является PASS по данным.

## 5. Открытые условия

- Семантика `F_FULLFSYNC` подтверждена локальной man-страницей `fcntl`, но namespace durability и исполнение на выбранном накопителе не подтверждены ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); уровни 2–3 не выполнялись.
- Модель накопителя/контроллера внутреннего SSD не зафиксирована.
- Стенд уровня 3 подготовлен и использован только для внешнего USB-накопителя (S9-39); внутренний SSD целевой комбинации не испытывался.
