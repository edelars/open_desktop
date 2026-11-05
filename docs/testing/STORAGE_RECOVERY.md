# Методика испытаний storage recovery

Статус: **черновик S9-00, ожидает ревью**. Документ описывает уровни испытаний и правила evidence. Ни одно испытание ещё не выполнялось: storage не реализован ([`od-storage`](../../crates/od-storage/src/lib.rs) — scaffold). Все уровни имеют статус `not_run`; уровень 3 — дополнительно `blocked` до подготовки стенда и разрешения пользователя.

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

Claim «durable после power loss» допустим только для комбинации с `passed` на уровне 3. До этого документация говорит «durable в модели уровня 0/1/2», явно перечисляя уровень.

## 4. Повреждения (corruption)

Повреждения вносятся отдельно от crash cuts: усечение, порча lengths/root/map/payload, wrong version/identity, duplicate/conflicting entries, циклы и превышение depth. Ожидаемый исход — явная ошибка с местом и типом либо salvage в новый target с отчётом о valid/unknown/lost ranges. Bounded fuzzing parser'а дополняет, но не заменяет доказательство данных: «ноль crashes parser» не является PASS по данным.

## 5. Открытые условия

- Host durability API не подтверждён ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); уровни 2–3 бессмысленны до выбора протокола S9-06.
- Модель накопителя/контроллера внутреннего SSD не зафиксирована.
- Стенд для уровня 3 не подготовлен.
