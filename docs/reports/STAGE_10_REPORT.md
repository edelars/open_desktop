# Этап 10. Отчёт: графика и ускорение через Metal

Статус отчёта: **промежуточный. Этап 10 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и проверками на ветке `feat/s9-stage` (исходная точка — коммит `ce87a74`, добавление плана этапа 10). Все задачи S10-NN на момент создания отчёта имеют статус `not_started`; создание этого файла, каталога или пустого модуля не закрывает ни одну задачу.

Связанные документы: [план этапа 10](../plans/STAGE_10.md), [этап 10 дорожной карты](../ROADMAP.md#этап-10-графика-и-ускорение-через-metal), [виртуальная плата](../specs/VIRTUAL_BOARD.md), [реестр проблем](../KNOWN_ISSUES.md), [отчёт этапа 9](STAGE_9_REPORT.md).

## 1. Сводка

Всего задач S10-00…S10-41 по плану: 34 (S10-00…S10-26, S10-30…S10-34, S10-40, S10-41).

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 0 | — |
| Ревью пройдено, задача выполнена по своему критерию | 2 | S10-00, S10-01 |
| Ревью пройдено (контракт, без кода) | 1 | S10-02 |
| Ревью пройдено (код od-machine) | 1 | S10-03 |
| `not_started` | 31 | остальные задачи, см. таблицу ниже |

Чекбоксы `[ ]` в плане не отмечены: по `STAGE_10.md` §0 отмечаются только после ревью.

## 2. Таблица задач

| Задача | Статус | Коммит | Ревью | Комментарий |
|---|---|---|---|---|
| S10-00 | ревью пройдено | 6fcce92 | субагент Opus, замечания учтены | Отчёт, плата, карта проекта |
| S10-01 | ревью пройдено | (см. коммит ниже) | субагент Opus, замечания учтены | Методика и эталоны |
| S10-02 | ревью пройдено (контракт, без кода) | (см. коммит ниже) | ревью 1: major 1, minor 8; ревью 2: blocker и major нет, minor учтены | Контракт дисплейного устройства |
| S10-03 | ревью пройдено (код в od-machine, тесты OK) | (см. коммит ниже) | ревью 1: blocker/major нет, minor 4 — исправлены, повторного ревью не было | Модель устройства в `od-machine` |
| S10-04 | not_started | — | — | `od-host` и C ABI |
| S10-05 | not_started | — | — | Транспорт кадров |
| S10-06 | not_started | — | — | Публикация кадров из VMHost |
| S10-07 | not_started | — | — | Окно и Metal view |
| S10-08 | not_started | — | — | Гостевой probe дисплея |
| S10-09 | not_started | — | — | Сквозной срез |
| S10-10 | not_started | — | — | Измерения пути кадра |
| S10-11 | not_started | — | — | Размер окна, масштаб, Retina |
| S10-12 | not_started | — | — | Привязка к PCI (blocked до S4-30 и решения о vendor ID) |
| S10-13 | not_started | — | — | Протокол display-only драйвера |
| S10-14 | not_started | — | — | Курсор |
| S10-15 | not_started | — | — | Несколько экранов |
| S10-16 | not_started | — | — | Ввод из окна (доставка в гостя blocked до xHCI/HID) |
| S10-17 | not_started | — | — | Решение по IOSurface по замерам |
| S10-18 | not_started | — | — | GOP этапа 7 (blocked до этапа 7) |
| S10-19 | not_started | — | — | Linux на GOP (blocked до этапов 5 и 7) |
| S10-20 | not_started | — | — | Windows Setup на GOP (blocked до этапов 7 и 8) |
| S10-21 | not_started | — | — | Linux display-only (blocked до этапа 5) |
| S10-22 | not_started | — | — | Windows display-only WDDM (blocked до этапа 8) |
| S10-23 | not_started | — | — | Динамическое разрешение и т. д. в Windows |
| S10-24 | not_started | — | — | Отказы транспорта и процесса |
| S10-25 | not_started | — | — | Длительный прогон ступени 2 |
| S10-26 | not_started | — | — | Промежуточное ревью G1–G3 |
| S10-30 | not_started | — | — | Scope ускоренного пути (после S10-23) |
| S10-31 | not_started | — | — | Контракт графического протокола |
| S10-32 | not_started | — | — | Контракт трансляции шейдеров |
| S10-33 | not_started | — | — | Контракт синхронизации и device loss |
| S10-34 | not_started | — | — | План ступеней 3–5 |
| S10-40 | not_started | — | — | Итоговое ревью ступеней 1–2 |
| S10-41 | not_started | — | — | Приёмка ступеней 1–2 |

## 3. Задачи

Разделы по задачам добавляются вместе с самой задачей. Раздел S10-00 ниже описывает подготовительные документы; его статус — ожидает ревью, по таблице выше он `not_started` до принятия.

### S10-00. Scope, отчёт и плата

- Файлы: этот отчёт; строка «Экран» в [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md) (ссылка на будущий `DISPLAY_DEVICE.md`, GOP и GPU acceleration не тронуты); [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md) (документы этапа 10 и fixtures `tests/fixtures/guest/stage10/`).
- Проверки: `python3 tools/check_layout.py` — 45 ошибок до и после изменения (базовый уровень, связан со ссылками на ignored `work/`).

### S10-01. Методика проверки изображения и эталоны

- Файлы: [docs/testing/DISPLAY.md](../testing/DISPLAY.md) (четыре уровня доказательств, допуски, инструменты); [tools/build/display_pattern.py](../../tools/build/display_pattern.py) (детерминированный генератор raw-узоров `gradient`, `bars`, `checker`, печатает SHA-256); [tests/system/display_compare.py](../../tests/system/display_compare.py) (сравнение raw-кадров, допуск на канал, коды выхода 0/1/2); [tools/build/test_display_pattern.py](../../tools/build/test_display_pattern.py) (11 unittest); эталоны и формулы в [tests/fixtures/guest/stage10/README.md](../../tests/fixtures/guest/stage10/README.md): `bars_64x48_seed0.raw` (SHA-256 `a738f2f3…a3e2`) и `checker_64x48_seed0x204080.raw` (`12779eb3…d529`), по 12 288 байт каждый.
- Проверки: `python3 -m unittest tools/build/test_display_pattern.py` — 11 тестов OK; повторная генерация даёт тот же хеш; сдвиг одного байта детектируется (exit 1); неверный размер даёт exit 2; `--help` у обоих скриптов работает.
- Ревью: независимый субагент (Opus, medium), замечания: blocker и major нет; minor (формулировки docstring, имя файла fixture с hex-seed, текст ошибки размера, покрытие тестами gradient/--help/списка отличий, описание кода 2) исправлены. Повторного ревью после minor-правок не проводилось; их покрывают новые тесты.
- Ограничения: методика проверена только на эталонах 64×48; уровни 2–3 ещё не применялись к кадрам VMHost или Metal.

### S10-02. Контракт дисплейного устройства

- Статус: ревью пройдено (два прохода; blocker и major нет). Кода нет. Открытые блокеры для S10-08 — dword MMIO (§13.1).
- Ревью 1 (архитектурное):
  - **major:** §7.2/§7.4 описывали код `feat/s9-stage`, который отстаёт от `main` на 20 коммитов (merge-base `336083a`). Условия безопасности «без callback» теперь сверены с `main`: page generations только для `Ram`; `write_bytes` проверяет и повышает поколения до копирования; `code_page_generation` и `tracked_code_snapshot` требуют `Ram`; JIT-путь `f300ef8`. Записано, что S10-03 сверяет эти условия с собственной базой, а правка «Committed writes» в `MACHINE_CONTRACT.md` делается по тексту `main`.
  - **minor, исправлены:**
    - A/D-эффекты walk при `DeviceError` (§9.2);
    - `COUNTER_EXHAUSTED` как шаг 5 в `MODE_COMMIT` (§5);
    - класс доступа «RO + doorbell» (§4);
    - `ACTIVE` в предикате «кадр есть» (§6, §10.1);
    - согласование `FB_BASE`/`FB_SIZE` с картой при сборке шины и типизированный handle (§7.3);
    - имя `BusAccessError::UnmappedPhysical`;
    - ширины 36/64 (§2.1);
    - блокер S10-08 (§13.1).
- Файлы: [DISPLAY_DEVICE.md](../specs/DISPLAY_DEVICE.md) (новый); строка «Экран» в [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md) — ссылка на контракт и фиксированные адреса.
- Конфигурация v1: один экран, `BGRX8888` (байты B,G,R,X; LE), stride = width × 4, до 4096×2304, `FB_SIZE` = 37 748 736 байт (36 MiB). Лимиты проверяются до умножения: после проверки произведение < 2^26; без проверки переполняется даже `u64`.
- Адреса без PCI: framebuffer `0xF800_0000` + 36 MiB, регистры MMIO `0xFC00_0000` + 4 KiB; диапазон `0xF800_0000–0xFC00_0FFF` зарезервирован под дисплей. Существующие карты `od-machine` его не пересекают; общей карты платы ещё нет — открытый вопрос.
- Регистры: 32 бита, LE, только dword-доступ. `WIDTH`/`HEIGHT`/`FORMAT` — запрос; `MODE_COMMIT` применяет режим атомарно или пишет код в `STATUS.COMMIT_ERROR`; `FRAME_GENERATION` — doorbell «кадр готов»; host не сканирует по таймеру. Внутренний `frame_mode_epoch` не даёт показать старую память в новом режиме.
- Backing: выбран вариант (б) — новый `MemoryRegionKind::Framebuffer`. Backing в `PhysicalMemory`, вне `RamLimits`, без committed-write callback. Fetch и page-table структуры в регионе запрещены (backend-ошибка), поэтому кеши кода и трансляций от его байтов не зависят. Вариант (а) отвергнут: callback на каждый пиксель, учёт в `RamLimits`, риск показать регион ОС как RAM. Места кода для S10-03 перечислены в §7.4 контракта.
- Ошибки: исправимые гостем (коды в `STATUS`, RO/резерв игнорируются) и backend (`DeviceError::UnsupportedWidth`/`Misaligned`/`UnsupportedSpace`, выход за окно — ошибка шины). Сверено с разделами «Синхронная MMIO/PIO bus» и «Committed writes» `MACHINE_CONTRACT.md` и с `enum DeviceError` в `bus.rs`.
- Источник формата: UEFI 2.11, GOP `PixelBlueGreenRedReserved8BitPerColor`. Страница uefi.org вернула 403, номер подраздела не перепроверен.
- **Блокер для S10-08** (не закрыт): tracked paged-путь CPU делает в MMIO только qword-записи, а контракт требует dword. Без dword MMIO-профиля probe не написать.
- Открытые вопросы (§13 контракта): подтвердить адреса после карты S4-00/S4-30; reset не сбрасывает `generation`; состав snapshot.

### S10-03. Модель устройства в `od-machine` — ревью пройдено

- Файлы: `crates/od-machine/src/display_device.rs` (новый: регистры, MODE_COMMIT, doorbell, счётчики, host handle `DisplayHandle`, `read_frame_into`); `crates/od-machine/tests/display_device.rs` (новый, 5 интеграционных тестов); `physical_memory.rs` (`MemoryRegionKind::Framebuffer`, backing вне `RamLimits`, без callback и page generations); `bus.rs` (согласование FB_BASE/FB_SIZE и окна регистров при сборке `MachineBus`, `DisplayBindingError`); `page_walk.rs`, `identity_memory_adapter.rs` (fetch и table read из Framebuffer — backend-ошибка); `lib.rs`; раздел «Display device (S10-03)» в `docs/specs/MACHINE_CONTRACT.md`.
- Проверки: `cargo test -p od-machine --offline` — все наборы OK (lib 99 тестов); `cargo check --workspace --all-targets --offline` — OK; `git diff --check` — чисто. `cargo fmt --all -- --check` падает только на `tools/power-loss/src/main.rs`, не тронутом в этой задаче.
- Ревью: независимый субагент (Opus, medium): blocker и major нет; minor исправлены (текст о типе ошибки fetch, проверка всех MMIO-привязок дисплея, checked-сравнение смещения, тесты приоритета ошибок размера над форматом). Повторного ревью после minor-правок не проводилось; их покрывают тесты.
- Отклонения и открытые вопросы: `current_mode()` возвращает структуру с нулевыми полями как «режима нет» (а не `Option`); `read_frame_into` принимает `&MachineBus`, `ModeChanged` в штатном вызове недостижим и проверяется через внутренний хук; handle не привязан к шине кодом (только документом); блокер S10-08 (dword MMIO в tracked paged-пути) не тронут.
- Не сверено с `main`: пункты §7.4 (а)–(д) контракта (page generations, `tracked_code_snapshot`, JIT-путь od-host). На этой ветке эквивалентных механизмов нет; тестов «code-API отвергает Framebuffer» нет.

## 4. Открытые ограничения

- Этапы 5, 7 и 8 не начаты: GOP, Linux и Windows display-only задачи заблокированы.
- По плану PCI-привязка (S10-12) зависит от этапа 4 (S4-30); первый срез планируется с фиксированными адресами без PCI.
- Ускоренный путь (ступени 3–5) не начат и зависит от принятой ступени 2 и Windows-драйвера.
