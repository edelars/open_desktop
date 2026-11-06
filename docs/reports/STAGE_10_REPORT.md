# Этап 10. Отчёт: графика и ускорение через Metal

Статус отчёта: **промежуточный. Этап 10 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и проверками на ветке `feat/s9-stage` (исходная точка — коммит `ce87a74`, добавление плана этапа 10). Все задачи S10-NN на момент создания отчёта имеют статус `not_started`; создание этого файла, каталога или пустого модуля не закрывает ни одну задачу.

Связанные документы: [план этапа 10](../plans/STAGE_10.md), [этап 10 дорожной карты](../ROADMAP.md#этап-10-графика-и-ускорение-через-metal), [виртуальная плата](../specs/VIRTUAL_BOARD.md), [реестр проблем](../KNOWN_ISSUES.md), [отчёт этапа 9](STAGE_9_REPORT.md).

## 1. Сводка

Всего задач S10-00…S10-41 по плану: 34 (S10-00…S10-26, S10-30…S10-34, S10-40, S10-41).

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 0 | — |
| Ревью пройдено, задача выполнена по своему критерию | 2 | S10-00, S10-01 |
| Ревью пройдено (контракт, без кода) | 2 | S10-02, S10-05 (2 прохода) |
| Ревью пройдено (код od-machine) | 1 | S10-03 |
| Ревью пройдено (код od-host + ABI) | 1 | S10-04 |
| Ревью пройдено (код VMHost) | 1 | S10-06 |
| Ревью 1 учтено, повторное независимое ревью недоступно (отказ автоматического режима); самопроверка терминального пути | 1 | S10-07 |
| `not_started` | 27 | остальные задачи, см. таблицу ниже |

Чекбоксы `[ ]` в плане не отмечены: по `STAGE_10.md` §0 отмечаются только после ревью.

## 2. Таблица задач

| Задача | Статус | Коммит | Ревью | Комментарий |
|---|---|---|---|---|
| S10-00 | ревью пройдено | 6fcce92 | субагент Opus, замечания учтены | Отчёт, плата, карта проекта |
| S10-01 | ревью пройдено | (см. коммит ниже) | субагент Opus, замечания учтены | Методика и эталоны |
| S10-02 | ревью пройдено (контракт, без кода) | (см. коммит ниже) | ревью 1: major 1, minor 8; ревью 2: blocker и major нет, minor учтены | Контракт дисплейного устройства |
| S10-03 | ревью пройдено (код в od-machine, тесты OK) | (см. коммит ниже) | ревью 1: blocker/major нет, minor 4 — исправлены, повторного ревью не было | Модель устройства в `od-machine` |
| S10-04 | ревью пройдено (код od-host + ABI) | (см. коммит ниже) | ревью 1: minor 3 — исправлены; повторного ревью не было | `od-host` и C ABI; см. раздел S10-04 |
| S10-05 | ревью пройдено (2 прохода; контракт, без кода) | — | ревью 1: major 2, minor 5 — исправлены; ревью 2: blocker и major нет, minor 2 — исправлены без повторного ревью | Транспорт кадров; см. раздел S10-05 |
| S10-06 | ревью пройдено (2 прохода) | — | первое ревью: blocker нет, major 1 (отчёт), minor 6 — исправлены, повторного ревью не было | Публикация кадров из VMHost; см. раздел S10-06 |
| S10-07 | ревью 1 учтено, повторного ревью нет | — | — | Окно и Metal view; дамп offscreen совпадает с эталоном (tolerance 0); окно глазами не проверено |
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

### S10-04. Кадр через `od-host` и C ABI — ревью пройдено

- Файлы: `crates/od-host/src/display.rs` (новый: opaque handle, `od_display_session_create/destroy/mode/read_frame/step`, 12 unit-тестов); `crates/od-host/src/lib.rs` (одна строка `mod display;`); `crates/od-host/Cargo.toml` и `Cargo.lock` (path-зависимость `od-machine`); `include/open_desktop/display_abi_v1.h` (новый); `docs/specs/DISPLAY_HOST_ABI_V1.md` (новый). `host_abi.h`, `session_abi_v1.h` и их реализации не тронуты.
- **Первое production-использование `od-machine` из `od-host`:** добавлена зависимость `od-machine = { path = "../od-machine" }`; цикла нет (`od-machine` зависит только от `od-isa`).
- Решения: handle — никогда не переиспользуемый идентификатор из процессного реестра (не указатель), поэтому повторный destroy и подделанный handle дают `INVALID_HANDLE` детерминированно; перекрывающиеся вызовы на одном handle — `BUSY`; panic под `catch_unwind` — `INTERNAL_ERROR`, сессия помечается неисправной; все структуры несут `abi_version`, `struct_size` (в том числе выходные: заполняются вызывающим до вызова); `step` для профиля 1 — `OK` + `outcome = NOTHING_TO_EXECUTE`. Профиль 2 в документе зарезервирован за S10-08, в коде пока `UNSUPPORTED_PROFILE`. **Решение (координатор):** профиль 1 — единственный test-only профиль; при `create` он сам программирует режим 64×48 BGRX (stride 256) и рисует узор `bars` seed 0 (тот же алгоритм, что `display_pattern.py`; кадр равен фикстуре `bars_64x48_seed0.raw`, SHA-256 `a738f2f3…a3e2`) через регистры и память устройства. Test-only функций в ABI нет; тест `od-host` сравнивает кадр с файлом фикстуры.
- Проверки (2026-11-06): `cargo test -p od-host --offline` — 12 новых тестов `display::tests` OK; `memory_corpus` (22) и `stage3_dword_store_corpus` (5) OK; в lib-наборе 299 OK и **21 FAILED** (`MappingAlreadyLive`, JIT-тесты `s3_20*` и `native_register_v1_corpus`) — те же 21 падений при `--skip display` и при `--test-threads=1`, код этих тестов не менялся; причина не исследована, не чинилась. `cargo check --workspace --all-targets --offline` — OK; `cc -fsyntax-only -x c include/open_desktop/display_abi_v1.h` (и как C++) — OK; `git diff --check` — чисто; `cargo fmt --all -- --check` падает только на `tools/power-loss/src/main.rs`.
- Не проверено: вызов из Swift, многопоточность, профиль 2, производительность копии.

### S10-05. Транспорт кадров VMHost → UI — ревью пройдено (2 прохода)

- Файлы: [protocols/control/FRAME_TRANSPORT_V1.md](../../protocols/control/FRAME_TRANSPORT_V1.md) (новый, нормативный); обзорная ссылка в [docs/specs/CONTROL_PROTOCOL_V1.md](../specs/CONTROL_PROTOCOL_V1.md). Control v0/v1 не изменены. Кода нет.
- Решения:
  - Память создаёт VMHost: `shm_open` со случайным именем, сразу `shm_unlink`, один `ftruncate`. Дескриптор передаётся один раз сообщением `ANNOUNCE` с `SCM_RIGHTS`; путь и имя не передаются.
  - Слотов 2…4, рекомендация 3 — гипотеза до S10-10. Слот кратен 16 KiB, наибольший — 36 MiB. Максимум памяти — 151 011 328 байт, меньше 2^28.
  - Seqlock на `generation` описателя (`0` — слот недействителен). Writer: relaxed-запись `0`, `fence(release)`, пиксели и поля, release-запись `N`. Reader: acquire-чтение, затем каждое поле описателя ровно один раз relaxed-чтением в локальную копию. Проверки и копирование идут только по копиям, без гонки и TOCTOU. Затем копия пикселей, `fence(acquire)`, relaxed-перечитывание. `reader_claim` — подсказка, корректность от неё не зависит. Swift выполняет атомарные операции через C-shim со встроенными функциями clang `__atomic_*`.
  - Уведомления идут отдельным протоколом `ODFT` v1 (`VERSION`, `HELLO`, `ANNOUNCE`, `MODE`, `FRAME`) по тому же унаследованному `socketpair`, fd 3, в дисплейном режиме VMHost. Control v1 не расширялся: его текст запрещает неизвестные виды и профили без смены версии. На `HELLO` с чужой версией VMHost отвечает 16-байтным заголовком `VERSION` со своей версией и закрывает соединение. UI переходит в `INCOMPATIBLE` и показывает версию; закрытие без заголовка — `DISCONNECTED`.
  - Состояния reader-а: `CONNECTING`, `NO_SIGNAL`, `MODE_CHANGING`, `SHOWING`, `DISCONNECTED`, `INCOMPATIBLE`. `ANNOUNCE` переводит в `NO_SIGNAL`. `MODE_CHANGING` используется только после показанного кадра.
  - Writer сравнивает `mode_change_count` кадра с последним `MODE`, поставленным в очередь, до публикации. Кадр старого режима не публикуется. `EAGAIN` у `sendmsg(ANNOUNCE)` повторяется тем же `sendmsg` с ancillary.
- Проверки: таблица размеров и смещений пересчитана скриптом (2, 3 и 4 слота максимального размера, test pattern 64×48 с 3 слотами). Все 32-битные поля помещаются в свою ширину. `git diff --check` — чисто. `python3 tools/check_layout.py` — 45 ошибок, базовый уровень, новых нет; ссылки нового документа битых не дают.
- Ревью 2 (повторное): blocker и major нет. Два minor исправлены без повторного ревью: условие `NO_SIGNAL`/`MODE_CHANGING` через флаг `shown_once` (§7.2) и статус в сводке отчёта.
- Ревью 1 (архитектурное): blocker нет, major 2, minor 5 — все исправлены.
  - major: атомарное однократное чтение полей описателя (§5.2, §8.3);
  - major: достижимость `INCOMPATIBLE` (§8.1, §7.1, §7.2);
  - minor: таблица состояний и переходы (§7.2);
  - minor: сравнение с последним `MODE` в очереди до публикации (§5.1, §6.3);
  - minor: `EAGAIN` у `sendmsg(ANNOUNCE)` (§4.3, §6.4);
  - minor: `SIGBUS` как явное исключение v1 (§8.3);
  - minor: допущение о порядке через системный вызов (§5.4).
- **Исключение v1 до S10-24:** UI может получить `SIGBUS`, если неисправный VMHost уменьшит объект повторным `ftruncate` (в macOS нет аналога `F_SEAL_SHRINK`). S10-24 закрывает исключение тестом, что macOS отвергает повторный `ftruncate`, или обработкой. VMHost от действий UI `SIGBUS` не получает: объект unlinked и живёт, пока отображён.
- Допущение v1: порядок «запись в память → сообщение в сокете» обеспечивает системный вызов. По C11 такая пара формально не синхронизируется (§5.4); проверяется в S10-06/S10-24.
- Открытые вопросы:
  - поведение `fstat` и повторного `ftruncate` для POSIX shm на macOS — источник не найден, проверяется тестами S10-06/S10-24;
  - физический объём памяти слотов 36 MiB при малом режиме не измерен;
  - каналы для одновременной работы control и дисплея — после этапа 10.

### S10-06. Публикация кадров из VMHost — ревью пройдено (2 прохода)

- Файлы: `apps/vmhost/Sources/FrameTransport.swift` (новый: writer транспорта v1, test-only `DisplaySmoke`); `apps/vmhost/Sources/FrameTransportAtomics.h` (новый: C-шим atomics §5.3); `apps/vmhost/Sources/VMHostABI.h` (include `display_abi_v1.h` и шима, обёртки `UINT32_C`-констант и `shm_open`, потому что Swift не импортирует вариадический `shm_open`); `apps/vmhost/Sources/main.swift` (ветка `--display-smoke`); `tests/system/display_transport_smoke.py` (новый); `apps/vmhost/README.md` (сборка и запуск); `protocols/control/FRAME_TRANSPORT_V1.md` §6.5 (один абзац о статусе завершения процесса).
- Реализовано: shm по §2.2 (случайное имя, `shm_unlink`, один `ftruncate`, `FD_CLOEXEC`), раскладка и заголовок по §3 (3 слота, 64×48 → `total_size` 65 536), публикация по seqlock §5.1, выбор слота §6.2 (не `front`, не слот из `reader_claim`), `ANNOUNCE` с `SCM_RIGHTS` одним `sendmsg` (EAGAIN/EINTR повторяют тот же `sendmsg`, результат 0 — ошибка), очередь `MODE`/`FRAME` §6.4, `VERSION` на `HELLO` с чужой версией, проверка `MODE`/кадра на `max_*` и формат до постановки в очередь (§6.3), порядок проверок §3.5 с UInt64. Кадр копируется один раз (`od_display_session_read_frame` прямо в слот). Режим `--display-smoke pattern` — **test-only**, включается только явным флагом; параметры `--display-poll-hz` (по умолчанию 60) и `--display-hold-until-claim 1` (держит публикацию, пока reader не запишет `reader_claim`).
- **Отклонение от плана, шаг 3:** узор берётся из профиля `od-host` `OD_DISPLAY_PROFILE_TEST_PATTERN` (он сам рисует `bars` seed 0 при `create`), а не из Swift-копии `display_pattern.py`. Swift-копии алгоритма нет.
- Статус завершения процесса (не часть wire format): EOF и ответ `VERSION` — 0; ошибка протокола UI (чужой magic, неверный `HELLO`, байты после `HELLO`) отказ хоста и любая ошибка сокета, кроме обычного EOF — ненулевой.
- Проверки (2026-11-06, `feat/s9-stage`): `cargo build -p od-host --offline` — OK; команда `xcrun swiftc` из README (с `target/debug/libod_host.a`) — OK без предупреждений; сборка `vmhost_generated_fault_helper` с новым `VMHostABI.h` — OK; `python3 tests/system/display_transport_smoke.py` — код 0, `differing_pixels=0` (tolerance 0), 3 прогона подряд после правок по ревью (до них ещё 6); `python3 -m py_compile` — OK; `git diff --check` — чисто; `python3 tools/check_layout.py` — 45 ERROR, как в базе.
- Регрессия обычного пути VMHost после правки `main.swift` (2026-11-06; `target/od-vmhost-smoke`, launcher и generated-fault helper собраны командами README с release-библиотекой `target/release/libod_host.a`): `vmhost_control_v1_frames_smoke.py`, `vmhost_control_v1_code_mismatch_smoke.py`, `vmhost_control_v1_isolation_smoke.py`, `vmhost_control_v1_run_id_exhaustion_smoke.py`, `vmhost_control_v1_run_stop_id_smoke.py`, `vmhost_control_v1_same_code_smoke.py`, `vmhost_control_v1_stop_eof_smoke.py`, `vmhost_isolation_smoke.py`, `vmhost_result_state_smoke.py`, `vmhost_launcher_signal_smoke.py`, `vmhost_generated_fault_smoke.py` — код 0 (первые восемь и result_state на debug-библиотеке тоже 0; generated-fault на debug 3 прогона из 4 прошли). `vmhost_control_race_smoke.py` — **код 1** («unsupported version: VMHost sent a response after rejecting the frame»); тот же код 1 и то же сообщение даёт VMHost, собранный из исходников `HEAD` без моих правок (2 прогона у каждого), поэтому это не регрессия S10-06, а существующий отказ, причина не исследована и не чинилась. На debug-библиотеке `vmhost_launcher_signal_smoke.py` падал («helper did not start»), на release проходит. Скрипт `tests/system/vmhost_app_bundle_smoke.sh` не запускался (требует отдельной сборки bundle, к S10-06 отношения не имеет).
- Smoke покрывает: `fstat` до `mmap`; поколение слотов 0 до открытия гейта и ненулевое после (переход 0 → N детерминирован, без таймера); после открытия гейта кадр лежит в слоте 1 (слот 0 занят `reader_claim`, проверка `slot == 1`); перепроверка поколения при чтении; ответ `VERSION` на `HELLO` v2; ненулевой код и текст «frame transport protocol» в stderr при чужом magic, неверном `HELLO` и байте после `HELLO`; код 0 по EOF.
- Дизассемблирование шима проверено: (а) `clang -O2 -arch arm64 -S` на обёртках шима даёт `ldapr`, `stlr`, `str`, `ldr`, `dmb ish`, `dmb ishld` по таблице §5; (б) в собранном `target/od-vmhost-smoke` (`otool -arch arm64 -tV`) функция `publishFrame` содержит `dmb ish` (шаг 3 §5.1), `stlr` (release-публикация `generation`) и relaxed `ldr` `reader_claim` по смещению 0x40.
- Не проверено / ограничения:
  - Профиль 1 не меняется после `create`: generation навсегда 1, смены `MODE` и второго кадра нет, поэтому эти переходы writer-а (в том числе `C > Q`, отбрасывание при `C < Q`, замена в очереди) smoke не выполняет.
  - Поведение `fstat` для POSIX shm и повторного `ftruncate` на macOS по источникам не подтверждено; smoke лишь проверил, что `st_size` равен `total_size` на этой машине. Допущение порядка через syscall (§5.4) тестом не подтверждено, только 9 успешных прогонов без стресса.
  - Очередь с частичной отправкой и EAGAIN у `ANNOUNCE` не нагружалась (сокет не переполнялся).
  - Библиотека собрана в debug; замеров производительности нет (S10-10).

### S10-07. Окно и Metal view — ревью 1 учтено, повторного независимого ревью нет

- Файлы: `apps/macos/Sources/FrameReader.swift`, `MetalPresenter.swift`, `DisplayWindow.swift`, `VMHostLaunch.swift`, `display_viewer_main.swift` (новые); `apps/macos/README.md` (новый); `tests/system/display_viewer_dump_smoke.py` (новый); `launcher.swift` (общий код запуска вынесен в `VMHostLaunch.swift`, поведение и тексты ошибок те же); `apps/vmhost/README.md` и `tests/cpu/stage3_harness.py` (в команду сборки лаунчера и списки исходников добавлен `VMHostLaunch.swift`). Файлы VMHost не менялись; шим `FrameTransportAtomics.h` общий, без копии. Просмотрщик не линкует `libod_host.a`.
- Проверки (2026-11-06): сборка просмотрщика по README — OK; `display_viewer_dump_smoke.py` 3 раза — код 0, `differing_pixels=0` (tolerance 0, 1:1); `display_transport_smoke.py` — код 0; `vmhost_control_v1_frames_smoke`, `vmhost_launcher_signal_smoke`, `vmhost_result_state_smoke`, `vmhost_control_v1_same_code_smoke`, `vmhost_control_v1_stop_eof_smoke` — код 0 (лаунчер и VMHost на release-библиотеке); окно с `--close-after-ms 6000` — код 0; `git diff --check` чисто; `check_layout.py` — 45 ERROR (без роста).
- `vmhost_isolation_smoke.py`: код 0 в 7 из 9 запусков (в том числе 6 подряд на release-VMHost); 2 запуска дали код 1 с сообщением "surviving VMHost returned RAX=546, reason=2" (один на release, один на debug-VMHost). Скрипт лаунчер не использует, файлы VMHost не менялись, поэтому причина — временная нестабильность теста, не правка S10-07.
- `vmhost_control_race_smoke.py` падает ("unsupported version: VMHost sent a response after rejecting the frame") и с лаунчером из HEAD без правок S10-07 — отказ не вызван этой задачей, причина не исследована.
- `test_stage3_harness`: 7 падений из 152 (`152 != 151`, `326 != 325`, `152 != 149`: закреплённое число тестов), те же 7 имён на чистом снимке HEAD (`git archive HEAD` в `work/tmp/head`) и в рабочем дереве. Новых падений нет.
- Правки после ревью (2026-11-06): оконный режим возвращает ненулевой код при ошибке запуска, при `DISCONNECTED`/`INCOMPATIBLE` и при статусе VMHost ≠ 0 (`exit(code)` после `NSApp.terminate`); при терминальном состоянии reader делает munmap/close, затем останавливает VMHost (EOF до 2 с, SIGTERM, 500 мс, SIGKILL, reap), реальный статус кэшируется и показывается в окне; граница cmsg берётся по `msg_controllen`; пул до 3 запасных `MTLTexture` (повторное использование, если текстуру не читает in-flight command buffer; размер пула измерить в S10-10); смок запускает просмотрщик в своей группе процессов и проверяет, что VMHost ушёл. Проверено: VMHost убит во время работы окна — код 1, статус 137 в stderr, зомби нет; `dump` смок 3 раза код 0; `display_transport_smoke.py` код 0.
- **Ручной показ окна и скриншот НЕ проверены** (план S10-07 требует скриншот окна): `screencapture` в песочнице не создаёт изображение, окно глазами никто не видел; проверено только отсутствие падения и коды выхода. Нужен ручной скриншот до приёмки.
- Не проверено: показ окна глазами и скриншот (`screencapture` в песочнице недоступен); состояния `MODE_CHANGING`, `INCOMPATIBLE`, `DISCONNECTED` в окне и смена поколения/режима (тестовый профиль их не порождает); масштаб, пропорции и Retina (S10-11); измерения (S10-10).

## 4. Открытые ограничения

- Этапы 5, 7 и 8 не начаты: GOP, Linux и Windows display-only задачи заблокированы.
- По плану PCI-привязка (S10-12) зависит от этапа 4 (S4-30); первый срез планируется с фиксированными адресами без PCI.
- Ускоренный путь (ступени 3–5) не начат и зависит от принятой ступени 2 и Windows-драйвера.
