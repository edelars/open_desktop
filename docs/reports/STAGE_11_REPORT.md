# Этап 11. Отчёт: интеграция с macOS и качество настольного продукта

Статус отчёта: **промежуточный. Этап 11 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и проверками на ветке `feat/s9-stage` (исходная точка — коммит `5e9e4e9`, добавление плана этапа 11). Все задачи S11-NN, включая S11-00 (реализован в рабочем дереве, ожидает ревью, не принят), имеют в таблице статус `not_started`; создание этого файла, ADR, каталога или пустого модуля не закрывает ни одну задачу. Любой результат на профилях без гостевой ОС помечается «test-only, без гостя».

Связанные документы: [план этапа 11](../plans/STAGE_11.md), [этап 11 дорожной карты](../ROADMAP.md#этап-11-интеграция-с-macos-и-качество-настольного-продукта), [ADR-0006](../decisions/ADR-0006-desktop-app-shell.md), [архитектура](../ARCHITECTURE.md), [карта проекта](../PROJECT_LAYOUT.md), [состояние VM](../specs/VM_STATE.md), [реестр проблем](../KNOWN_ISSUES.md), [отчёт этапа 10](STAGE_10_REPORT.md).

## 1. Сводка

Всего задач S11-00…S11-71 по плану: 47 (S11-00…S11-04, S11-05a/b/c, S11-06…S11-14, S11-20…S11-30, S11-31a…e, S11-32…S11-34, S11-40…S11-43, S11-50, S11-51, S11-60…S11-62, S11-70, S11-71).

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 0 | — |
| Ревью пройдено | 0 | — |
| `not_started` | 47 | все задачи, включая S11-00, см. таблицу ниже |

S11-00 в таблице остаётся `not_started`: принятие — только после архитектурного ревью. Отдельно: **S11-00 реализован в рабочем дереве, ожидает ревью, не принят** (документы без кода; подробности — в разделе S11-00).

Чекбоксы `[ ]` в плане не отмечены: по `STAGE_11.md` §0 отмечаются только после ревью по правилам `AGENTS.md`. Gates G1–G7 не приняты.

## 2. Таблица задач

Статус S11-00 в таблице остаётся `not_started` до архитектурного ревью; фактическое состояние («реализован в рабочем дереве, ожидает ревью, не принят») — в разделе S11-00 ниже.

| Задача | Статус | Коммит | Ревью | Название | Зависимости / blocked (по плану §6) |
|---|---|---|---|---|---|
| S11-00 | not_started | — | — | Зафиксировать scope, завести отчёт, принять решения ADR-0006 и обновить карту | — |
| S11-01 | not_started | — | — | Написать методику проверки приложения и lifecycle | S11-00 |
| S11-02 | not_started | — | — | Контракт bundle VM и каталога машин | S11-00 |
| S11-03 | not_started | — | — | Нормативный control protocol v2: жизненный цикл VM | S11-00, S11-02 |
| S11-04 | not_started | — | — | Lifecycle v2 в VMHost на test-only профилях без гостя | S11-03 |
| S11-05a | not_started | — | — | Swift Package: цели, перевод тестов, паритет бинарников | S11-00 |
| S11-05b | not_started | — | — | Сборка `OpenDesktop.app`, entitlements и `Info.plist` | S11-05a |
| S11-05c | not_started | — | — | Перевод smoke и README на новую сборку | S11-05b |
| S11-06 | not_started | — | — | Библиотека машин: список, запуск, пауза, остановка, удаление, события | S11-02, S11-04, S11-05a |
| S11-07 | not_started | — | — | C ABI создания и проверки RAW-образа | S11-02 |
| S11-08 | not_started | — | — | Мастер создания VM и команда `PREPARE_DISK` | S11-06, S11-07 |
| S11-09 | not_started | — | — | Каталог ошибок и их показ | S11-03, S11-06 |
| S11-10 | not_started | — | — | Окно дисплея и ввод внутри приложения | S11-06 |
| S11-11 | not_started | — | — | Выход из приложения, сбой VMHost и повторный запуск | S11-06, S11-09 |
| S11-12 | not_started | — | — | Сон/пробуждение Mac, смена дисплеев, нехватка места и памяти | S11-06, S11-10 |
| S11-13 | not_started | — | — | CLI `odctl` на том же контракте | S11-06, S11-08, S11-09 |
| S11-14 | not_started | — | — | Сквозной срез оболочки и измерения | S11-05c, S11-08, S11-10, S11-11, S11-13 |
| S11-20 | not_started | — | — | ADR-0007 и протокол гостевого агента | S11-00; подтверждение владельца |
| S11-21 | not_started | — | — | Устройство канала агента в `od-machine` и ABI | S11-20; согласование с S4-24 |
| S11-22 | not_started | — | — | Сервис агента в VMHost: handshake, время, shutdown | S11-04, S11-21 |
| S11-23 | not_started | — | — | Freestanding probe агента — сквозной срез без ОС | S11-21, S11-22; blocked до S10-09 (сессия «CPU + устройства») |
| S11-24 | not_started | — | — | Буфер обмена (текст): host-часть | S11-22, S11-10 |
| S11-25 | not_started | — | — | Drag-and-drop файлов: контракт и host-часть | S11-22, S11-10 |
| S11-26 | not_started | — | — | Общие папки: контракт и host-сервис с защитой корня | S11-00, S11-03, S11-20 |
| S11-27 | not_started | — | — | Гостевой агент Windows | S11-22; blocked до этапа 8 |
| S11-28 | not_started | — | — | Гостевой агент Linux | S11-22; blocked до этапа 5 |
| S11-29 | not_started | — | — | Гостевые драйверы общих папок и сквозной срез | S11-26, S11-27/S11-28; blocked до этапов 5/8 |
| S11-30 | not_started | — | — | ADR-0008 и контракт сети | S11-00; подтверждение владельца по стеку |
| S11-31a | not_started | — | — | Trait backend, очереди, разбор L2/L3 и устойчивость к мусору | S11-30 |
| S11-31b | not_started | — | — | DHCP и DNS-пересылка | S11-31a |
| S11-31c | not_started | — | — | UDP и ICMP echo | S11-31a |
| S11-31d | not_started | — | — | TCP | S11-31a |
| S11-31e | not_started | — | — | Измерение NAT на loopback | S11-31b, S11-31c, S11-31d |
| S11-32 | not_started | — | — | Модель NIC, совместимая с Intel 82540EM | S11-30, S11-31a; blocked до S4-30 (PCI/ECAM) |
| S11-33 | not_started | — | — | Сквозной сетевой срез с Linux | S11-31e, S11-32; blocked до этапа 5 |
| S11-34 | not_started | — | — | Измерения сети с гостем | S11-33 |
| S11-40 | not_started | — | — | Класс аудиоустройства, правка платы и контракт | S11-00; решение владельца |
| S11-41 | not_started | — | — | Host backend звука с offline-проверкой | S11-40, S11-04 |
| S11-42 | not_started | — | — | Модель аудиоустройства | S11-40; blocked до S4-30 при PCI-устройстве |
| S11-43 | not_started | — | — | Сквозной аудиосрез | S11-41, S11-42; blocked до этапов 5/8 |
| S11-50 | not_started | — | — | Scope интеграции гостевых окон | blocked до S10-23 |
| S11-51 | not_started | — | — | Scope USB-классов и печати | S11-62 |
| S11-60 | not_started | — | — | Полный пользовательский сценарий | S11-14, S11-12 |
| S11-61 | not_started | — | — | Длительный прогон приложения | S11-14, S11-12 |
| S11-62 | not_started | — | — | Промежуточное ревью и приёмка gates G1–G3 | S11-14, S11-12, S11-13, S11-61 |
| S11-70 | not_started | — | — | Независимое итоговое ревью host-частей | S11-62, S11-22, S11-24, S11-25, S11-26, S11-31e, S11-40, S11-41 |
| S11-71 | not_started | — | — | Документация и приёмка этапа в достигнутых границах | S11-70 |

## 3. Задачи

Разделы по задачам добавляются вместе с самой задачей. Пустой раздел не означает выполнения.

### S11-00. Зафиксировать scope, завести отчёт, принять решения ADR-0006 и обновить карту

**Статус: реализован в рабочем дереве, ожидает архитектурного ревью, не принят** (в таблице — `not_started`). Коммита нет.

Файлы:

- новые: `docs/reports/STAGE_11_REPORT.md` (этот отчёт), [ADR-0006](../decisions/ADR-0006-desktop-app-shell.md) (статус «предложен, ждёт архитектурного ревью»; решение (б) — «подтверждение владельца: ожидается (да/нет/дата)»);
- изменены: `docs/PROJECT_LAYOUT.md` (текстовые строки: `ODShared`, `apps/cli/`, документы этапа 11; машиночитаемый перечень каталогов не тронут), `apps/AGENTS.md` (строки про `cli/Sources/` и способ сборки), `docs/plans/STAGE_11.md` (только строка статуса в S11-00 и строка глоссария про bookmark).
- не создавались: `Package.swift`, каталоги `apps/cli/` и `apps/macos/Sources/Shared/`, Swift-файлы.

Проверки: `python3 tools/check_layout.py` — 45 ошибок до и после (новых нет); `git diff --check` чист; локальные ссылки новых и изменённых документов существуют. Не проверено: `graphify update .` не запускался (кода нет); повторное ревью проведено (блокеров нет); замечания второго ревью — граф зависимостей `ODShared`/`ODAtomics`/`od-vmhost` и косметика — исправлены.

Результат независимого ревью: 12 замечаний, часть исправлена в документах, часть оформлена как открытые вопросы и предлагаемые правки плана, которые не вносились в рамках S11-00:

- исправлено в документах: согласован статус S11-00 (отчёт, строка статуса плана); в ADR — объяснение ограниченной передачи дескриптора против правила §4; `ODAtomics` содержит только `FrameTransportAtomics.h`, `VMHostABI.h` отнесён к `od-vmhost`; утверждения про независимость дескриптора от TCC/песочницы и про достаточность обычного bookmark переформулированы как гипотезы S11-04; lock `<bundle>/lock` описан как проектное утверждение (появится в S11-02/S11-04); состав `ODShared` приведён к S11-05a; пометки «создаётся в S11-05a» и «создаётся в S11-20» в карте проекта и `apps/AGENTS.md`; строка глоссария `STAGE_11.md` про bookmark; терминология в ADR.
- открытые вопросы для архитектурного ревью (нормативные документы не менялись): уточнить ARCHITECTURE §4 (дескриптор с ролью в типе сообщения); обновить ARCHITECTURE §2 и §9 (Xcode), кто и когда — после принятия ADR; как `odctl run` без UI получает ISO и диски вне bundle (см. список для S11-13 ниже).
- предлагаемые правки плана, ждут решения владельца по (б): S11-02 (формат `config.plist`, `boot_media` и `disks` как bookmark — без непроверенных утверждений про доступ), S11-04 и S11-06 (smoke перезапуска: доступ по bookmark и TCC, каталог `~/Documents/Open Desktop/`), S11-00 (обоснование в пункте (б) «доступ по bookmark привязан к процессу приложения»); S11-05a — убрать `VMHostABI.h` из `ODAtomics` и отнести к `od-vmhost`; S11-00 — добавить в список `ODShared` (`HIDUsage`, `InputTranslation`, `Viewport`) для согласованности с S11-05a.

### S11-01. Написать методику проверки приложения и lifecycle

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено, **не принят** (архитектурного принятия нет).

- Файл: [DESKTOP_APP.md](../testing/DESKTOP_APP.md) (новый). Содержит пять уровней доказательств с pass-критериями и командами (уровни 2–3 — «появится в S11-NN»), ручной чеклист UI из 8 шагов с ожидаемым наблюдением и приложением, измерения (повторы, fingerprint, JSON), правила честности, ограничения (XCUITest не обещается, пока нет Xcode project).
- Ревью: первое — 9 замечаний, исправлены; повторное — блокеров нет; минорные замечания второго прохода (два подслучая удаления, fingerprint по плану, формулировка уровня 3, английские слова) исправлены после него и отдельно не перепроверены.
- Проверки: `python3 tools/check_layout.py` — в файле ошибок нет; локальные ссылки существуют; `grep` на пробелы и табы пуст.
- Не проверено: команды документа не запускались (уровни 2–3 ещё не существуют); предложения, не из плана (формат JSON, часть fingerprint, правило про скриншоты), помечены в тексте и ждут решения владельца; трактовка «прогрев входит в 5 или нет» ждёт подтверждения.
- Открытые ограничения записаны в тексте документа; в `KNOWN_ISSUES.md` не вносились (это ограничения, не дефекты).

### S11-02. Контракт bundle VM и каталога машин

**Текущий статус (2026-11-06):** написан в рабочем дереве, ревью пройдено (четыре прохода), **не принят** (архитектурного принятия нет).

- Файл: [VM_BUNDLE.md](../specs/VM_BUNDLE.md) (новый). Раскладка `<Имя>.odvm/`; `lock` как системные вызовы `open`/`flock` (реализация для VMHost — открытый вопрос); схема `config.plist` v1 с полями (тип, лимит, умолчание, отсутствие, неизвестное поле); классы `valid`/`incompatible`/`invalid`; приоритет проверок при запуске; `Library.plist`; durability не подтверждена до S9-06 (ISSUE-055); bookmark как `data` с опциями, зависящими от решения (б) ADR-0006.
- Ревью: 1-й проход — 11 замечаний; 2-й — 11; 3-й — блокер (приложение и CLI не могут вызывать функции `od-host`) и 9 замечаний. Все исправлены; после блокера — узкая перепроверка без противоречий.
- Открытые вопросы — в §11 документа (17 пунктов): реализация захвата `lock` для VMHost, неполный bundle после отказа, Foundation-временные файлы, FIFO и симлинки в пути, имена классов ошибок (коды задаёт S11-03/S11-09), решение (б) ADR-0006.
- Проверки: ссылки и якоря существуют (скрипт); `grep` на пробелы и табы пуст; `check_layout.py` — ошибок про `VM_BUNDLE.md` нет.
- Не проверено: спецификация не запускалась (кода нет); лимиты — гипотезы без замеров; доступ VMHost к bundle под TCC — гипотеза до S11-04.
- Найдено вне документа: в `docs/KNOWN_ISSUES.md` два заголовка ISSUE-055 (стр. 17 и 1911); ссылка из VM_BUNDLE ведёт на первый. Не исправлялось (вне scope S11-02).
- Предложенная правка плана (не внесена): S11-03 п. 2 — «относительный путь внутри bundle» должен быть «имя внутри `disks/`»; S11-08 п. 2 — порядок `config.plist` последним (§5.1). Ждёт решения владельца.

### S11-03. Нормативный control protocol v2: жизненный цикл VM

**Текущий статус (2026-11-06):** написан `protocols/control/CONTROL_PROTOCOL_V2.md` (нормативный, открытые вопросы — §13), ревью пройдено (три прохода), **не принят** (архитектурного принятия нет); реализации нет, спецификация не запускалась.

- Содержание: кадр v2 (ширины полей, смещения, лимит 65 536 байт); `CONFIGURE` 1588 байт, пути только `<имя>` (по VM_BUNDLE §3.3); таблица состояние × операция (7×14); `ATTACH_RESOURCE` (один дескриптор, закрытый список ролей, `fstat`, без конвейеризации); `unsupported_durability` для `START`/`STOP`/`SHUTDOWN` при непустом `disks` и до S9-06 по VM_STATE §3; порядок проверок `SHUTDOWN`: `unsupported_durability` до `unsupported_no_agent`; 25 кодов `ERROR`; рукопожатие; поведение на мусор, усечённый кадр и EOF; backpressure вместо статуса 5.
- Решения, принятые в документе и подлежащие архитектурному ревью: невалидный UTF-8 и NUL внутри строки — не фатально, код `invalid_name`/`invalid_config`; `STOP` из `stopped` — `ACK ok` без `STATE_CHANGED`; `PAUSE reason = 6` (`disk_space`) для S11-12; `ACK in_progress` — исключение из «один терминальный кадр»; request ID строго последовательные.
- Расхождения с планом (исправить в плане, не в документе): S11-03 п. 2 «относительный путь внутри bundle» — должно быть `<имя>` (VM_BUNDLE §3.3, §11 п. 11); S11-03 п. 4 и S11-04 п. 3 — `SHUTDOWN` до S9-06 отвечает `unsupported_durability`, `unsupported_no_agent` — только после S9-06 (VM_STATE §3, план §0 п. 7); §4.5 плана — то же; S11-04 п. 1 — отказ `lock` приходит как `ERROR busy` с `failed_kind = 0`, а не `READY`; S11-04 п. 5 — неверная версия закрывает соединение без ответа (как v1).
- Изменён `protocols/control/FRAME_TRANSPORT_V1.md` (§2 п. 3): добавлена ссылка на fd 5 для `--display-fd`, как требует «Где» S11-03.
- Проверки: все относительные ссылки и якоря существуют (скрипт); `grep` на пробелы и табы пуст; `python3 tools/check_layout.py` — ошибок по документу нет.
- Не проверено: ничего не запускалось; числа (65 536, кольцо 64, `pause_timeout` 5000 мс, `output_drain_timeout` 5000 мс, `detail` ≤ 256, 21 дескриптор, `PROGRESS` 10/с) — гипотезы без замера; `SCM_RIGHTS` на macOS и TCC — гипотезы до S11-04.

### S11-04. Lifecycle v2 в VMHost на test-only профилях без гостя

**Текущий статус (2026-11-06):** реализовано в рабочем дереве, ревью пройдено (Opus: два прохода, замечания исправлены), **не принято** (архитектурного принятия нет).

- Файлы: `apps/vmhost/Sources/Lifecycle.swift` (машина состояний по CONTROL_PROTOCOL_V2 §7.2), `apps/vmhost/Sources/ControlV2Host.swift` (цикл poll, приём `SCM_RIGHTS`, очередь вывода 131 072 байта с backpressure, `STATS`, раннер display-сессии), `apps/vmhost/Sources/BundleLock.swift` (lock по VM_BUNDLE §2.1), `apps/vmhost/Sources/VMHostMain.swift` (режим `--control-v2`; v0, v1 и `--display-smoke` не менялись), `apps/macos/Sources/Shared/ControlV2.swift` (кодек в `ODShared`), тесты `apps/vmhost/Tests/LifecycleTests.swift` (10) и `apps/macos/Tests/ControlV2Tests.swift` (11), smoke `tests/system/vmhost_control_v2_lifecycle_smoke.py`. В `tests/cpu/stage3_harness.py` добавлены четыре новых файла в списки сборки VMHost.
- Реализовано: `HELLO/READY`; `CONFIGURE` с проверкой полей; `START`/`PAUSE`/`RESUME`/`STOP`/`QUERY`/`STATS`; `ATTACH_RESOURCE` (один дескриптор через `SCM_RIGHTS`, закрытие лишних, хранение по роли без использования); кольцо request ID на 64; `unsupported_durability` для непустого `disks` и для `SHUTDOWN` до S9-06; `unsupported` для сетевых, аудио, буфера обмена, `boot_media`, `shared_folders`.
- Не реализовано (по плану и спецификации): `PREPARE_DISK` (`unsupported` до S11-08), `SET_CLIPBOARD` (до S11-24), `PROGRESS`, `logs/vmhost.log` с ротацией, использование дескрипторов `ATTACH_RESOURCE`. Признак работы на профилях 1 и 4 без гостя — счётчик опросов в `STATS`; он растёт в `running` и замирает в `paused`.
- Расхождения с планом S11-04, решённые по нормативной спецификации CONTROL_PROTOCOL_V2 (записать в план при архитектурном ревью): (а) `SHUTDOWN` в `running` → `unsupported_durability`, `unsupported_no_agent` до S9-06 недостижим (план п. 3 требовал `unsupported_no_agent`); (б) lock занят → фатальный `ERROR busy` с `failed_kind = 0` вместо `READY`, выход со статусом 3 (план п. 1); (в) неверная версия → закрытие соединения без ответа, статус 4 (план п. 5); (г) имя диска только `<имя>` (план S11-03 п. 2); (д) lock реализован Swift-обёрткой над `open`/`flock` в `od-vmhost` без C ABI в `od-host` (план п. 1 говорил «через `od-host»; VM_BUNDLE §11 п. 12 — открытый вопрос).
- Пробелы спецификации, найденные при реализации: EOF до `HELLO` при неудачном lock — в реализации статус 4 по аналогии с мусором до HELLO (§4 п. 2 это не называет явно); EOF до `HELLO` при взятом lock — статус 0. Толкование «в полёте»: терминальный ответ считается поставленным, когда попал в очередь вывода; поэтому `operation_in_progress` и `in_progress` не отправляются. Записано в `apps/vmhost/README.md`.
- Проверки: `cd apps && swift build` и `swift build -c release` — exit 0; `swift test` — 36 тестов, 0 падений; `python3 tests/system/vmhost_control_v2_lifecycle_smoke.py` — exit 0, `ok` (5 запусков подряд в ходе работы, все exit 0; подпись не нужна); `python3 tools/check_layout.py` — ошибок по задаче нет; `grep` на пробелы в конце строк пуст.
- Не проверено: backpressure очереди вывода и таймаут дренирования 5000 мс живым прогоном; SIGTERM; `lock_changed` (гонка) и `io_error` по lock; `--input-fd` в v2-smoke не прогоняется (только в `--display-smoke`); порядок `running` на fd 3 относительно `ANNOUNCE` на fd 5 (§13 п. 11 спецификации); утечка лишних дескрипторов проверена косвенно (smoke «два дескриптора → статус 4» и код).

### S11-05a. Swift Package: цели, перевод тестов, паритет бинарников

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено, **не принят** (архитектурного принятия нет).

- Файлы: `apps/Package.swift` (новый, без внешних зависимостей); C-цели `ODAtomics` (`apps/macos/Sources/ODAtomics/`), `ODHostABI` (`apps/vmhost/Sources/ODHostABI/`), `ODHostConstants` (`apps/macos/Sources/ODHostConstants/`, отступление от плана — см. ниже); `ODShared` (`apps/macos/Sources/Shared/`, переносы через `git mv`, публичный API помечен `public`); `od-vmhost` (единственная цель, линкующая `libod_host.a`); `open-desktop-display-viewer`; `open-desktop-launcher-smoke`; тесты `OpenDesktopMacTests` и `OpenDesktopVMHostTests` на XCTest. `main.swift` переименован в `VMHostMain.swift`.
- Не объявлены: `OpenDesktop` и `odctl` (исходников нет; пустые цели запрещены).
- Тесты: `@main`-исполняемые файлы переведены в XCTest, проверки сохранены, 33 теста (25 Mac + 8 VMHost). Выбор XCTest: toolchain Swift 6.4 с XCTest.
- Отступления от плана (с причиной): (1) старые команды `xcrun swiftc` для трёх тестовых файлов заменены на `swift test --filter …` — файлы теперь `import XCTest` и через `swiftc` не собираются; (2) launcher-smoke зависит от отдельной C-цели `ODHostConstants` (только `host_abi.h`), а не от `ODHostABI`, чтобы не тянуть объявления движка; `libod_host.a` по-прежнему линкует только `od-vmhost`.
- Проверки (выполнены при закрытии ревью): `swift build` и `swift build -c release` — exit 0; `swift test` — exit 0, 33 теста без падений, `InputXCTests` печатает 2925454 проверок (как прежний бинарник); `swift test -c release` — exit 0; `tests/system/display_viewer_dump_smoke.py` — ok; в ходе работы исполнителя дополнительно зелёные: `display_viewer_scaled`, `display_transport`, `input_channel`, `cursor_overlay`, `vmhost_control_v1_same_code` (с бинарниками SwiftPM); `python3 tools/check_layout.py` — 45 ошибок (как до задачи, все про ссылки на `work/`).
- Известные падения, не связанные с пакетом: `vmhost_control_race_smoke.py` падает одинаково на старой и новой сборке (замер исполнителя; причина не исследована, в `KNOWN_ISSUES.md` не записано). `tests/cpu/test_stage3_harness.py` — 7 падений (`152 != 151` и подобные на `SELF_TEST_NAMES`); по проверке ревьюера они существовали до S11-05a (правки в `tests/cpu` затрагивают только пути). Записать в `KNOWN_ISSUES.md` отдельно — следующая задача.
- Не проверено: окна глазами; deployment target (SwiftPM берёт 12.0, прежние сборки шли с 26.0 — решение в S11-05b); подпись `od-vmhost` вручную (`codesign`/entitlements — S11-05b); другие машины.
- Затронутые файлы вне пакета: пути в `tests/cpu/*.py` и `protocols/control/INPUT_TRANSPORT_V1.md` (только пути), ссылки на `main.swift` в `docs/plans/STAGE_10.md`, `docs/plans/STAGE_11.md`, `docs/specs/VM_STATE.md`, `docs/KNOWN_ISSUES.md`; `docs/PROJECT_LAYOUT.md`, `docs/policies/DEPENDENCIES.md`, `apps/AGENTS.md`, `docs/decisions/ADR-0006-desktop-app-shell.md` (синхронизация с пакетом).

### S11-05b. Сборка `OpenDesktop.app`, entitlements и `Info.plist`

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено (Opus, замечания исправлены), **не принят** (архитектурного принятия нет).

- Файлы: `tools/build/build_app_bundle.py` (сборка `OpenDesktop` и `od-vmhost`, раскладка bundle, `plutil -lint`, ad-hoc подпись с `--options runtime`: сначала helper, затем приложение; проверка `codesign --verify --strict`); `apps/macos/Resources/Info.plist`; `apps/macos/Configuration/OpenDesktop.entitlements` (пустой: без sandbox и без JIT); `apps/vmhost/Configuration/vmhost.entitlements` (только `allow-jit`); `apps/macos/Sources/App/OpenDesktopApp.swift` и `AppOptions.swift` (окно с промежуточным текстом «Библиотека машин — S11-06», разбор `--vmhost`); `apps/macos/Tests/AppOptionsTests.swift` (6 тестов); цель `OpenDesktop` в `apps/Package.swift`.
- Путь helper `Contents/Helpers/od-vmhost` — соглашение; источник по структуре bundle не найден через Context7 (возможная ссылка — Apple TN2206, по памяти, не проверено).
- Решение, требующее подтверждения владельца: `platforms: [.macOS(.v14)]` и `LSMinimumSystemVersion` 14.0. Прежний минимум SwiftPM — 12.0 (проверено `vtool` на пустом пакете) — поднят для всей оболочки SwiftPM (viewer, launcher, VMHost). Прежние команды `swiftc` (под 26.0) совместимость не сужают.
- Расхождения с планом: `platforms` и unit-тесты добавлены сверх плана; `CFBundleIdentifier` `dev.opendesk.OpenDesktop` — рабочее значение до этапа 12; приложение находит helper через `Bundle.main.bundleURL`, а не `url(forAuxiliaryExecutable:)`.
- Проверки: `python3 tools/build/build_app_bundle.py` — exit 0 (`target/app-bundle/OpenDesktop.app`); `codesign --verify --strict --verbose=2` для приложения (`--deep`) и helper — exit 0 (ad-hoc, `adhoc,runtime`); у приложения пустые entitlements, у helper `allow-jit`; `swift build` — exit 0; `swift test` — 42 теста, 0 падений; launcher-smoke с helper из bundle — exit 0; приложение из bundle живёт ≥ 3 с, окно присутствует в `CGWindowList`; `python3 -m py_compile tools/build/build_app_bundle.py` — ok.
- Не проверено: окно глазами; запуск через Finder и `open`; macOS 14 (собирали и запускали на 26.7); Developer ID и нотарификация — этап 12; доступ к пользовательским файлам без sandbox (ADR-0006 (б), включая TCC).
- Пустое окно — промежуточный факт, не результат.

### S11-05c. Перевод smoke и README на новую сборку

**Текущий статус (2026-11-06):** реализован в рабочем дереве, **не принят**. Ревью подпрограммой не выполнено: запуск ревьюера был заблокирован ограничением среды; проверка сделана вручную (diff и запуск smoke).

- Smoke `tests/system/*.py` (17 файлов) используют бинарники SwiftPM по умолчанию (`apps/.build/debug/`); в diff меняются только пути, интерфейс (`nargs="?"`) и docstring. `vmhost_result_state_smoke.py`: `--diagnostics-dir` стал необязательным (временный каталог). `vmhost_app_bundle_smoke.sh` без аргументов собирает `build_app_bundle.py` и проверяет helper `Contents/Helpers/od-vmhost`; старый режим `LAUNCHER VMHOST ENTITLEMENTS [...]` сохранён для `tests/cpu/stage3_harness.py` (помечен «legacy»).
- README `apps/macos` и `apps/vmhost`, `docs/testing/DISPLAY.md`, `docs/testing/DESKTOP_APP.md` переведены на `swift build`/`swift test` и `build_app_bundle.py`; устаревшие пометки «появится в S11-04/S11-05a/S11-05b» обновлены. `vmhost_control_race_smoke.py` описан как падающий (ISSUE-058).
- Расхождения: (а) остался один блок `xcrun swiftc` для `tests/system/vmhost_generated_fault_helper.swift` — не цель SwiftPM, помечен «не нормативно»; убрать можно только добавлением цели в `apps/Package.swift`; (б) подписанный bundle проверяет только `vmhost_app_bundle_smoke.sh`, остальные smoke идут на неподписанном debug `od-vmhost` (JIT работает без hardened runtime); (в) `tests/cpu/stage3_harness.py` всё ещё вызывает `xcrun swiftc` (вне объёма).
- Проверки: `vmhost_app_bundle_smoke.sh` и 17 python-smoke с путями по умолчанию — exit 0 (по отчёту исполнителя); вручную повторно: `display_transport_smoke.py`, `vmhost_control_v1_same_code_smoke.py`, `vmhost_control_v2_lifecycle_smoke.py`, `vmhost_result_state_smoke.py` — exit 0; `swift build`/`swift test` — 42 теста; `python3 tools/check_layout.py` — 45 ошибок (как до задачи).
- Не запускались: `vmhost_control_race_smoke.py` (ISSUE-058), `tests/cpu/*` (ISSUE-060, ISSUE-061), smoke с окном и release-бинарники; macOS 14 не проверялась.

### S11-06. Библиотека машин: список, запуск, пауза, остановка, удаление, события

**Текущий статус (2026-11-06):** S11-06 реализован в рабочем дереве по частям: S11-06a (модель и клиент, пункты 1–3) закоммичен (`4789d7f`); S11-06b (библиотека и контроллер, пункты 4–6) реализован и ревьюирован, **не закоммичен на момент записи, не принят**. Ручной чеклист не выполнен (нет прав на запись экрана и assistive access).

- S11-06a: `BundleConfig.swift`, `Catalog.swift`, `ControlClientV2.swift` (ODShared), `VMHostLaunch.swift` (`display:`, `closeOtherDescriptors:`), тесты `BundleConfigTests`, `CatalogTests`, `ControlClientV2Tests`. Допущение: в `Library.plist` хранится путь `path` вместо bookmark (решение (б) ADR-0006 не подтверждено).
- S11-06b: `apps/macos/Sources/App/MachineController.swift` (состояние только по событиям клиента и exit status; кнопки по таблице §7.2; «Прервать запуск» в `starting`), `LibraryModel.swift` (каталог, открытые в этом запуске, удаление файлов с проверкой `.odvm` и регулярного `config.plist`), `LibraryView.swift`, `MachineRowView.swift`, `OpenDesktopApp.swift` (`--open <bundle>`, `--open=PATH`), `AppOptions.swift`; тесты `MachineControllerTests`, `AppOptionsTests`, включая реальный `od-vmhost` (запуск, пауза, продолжение, остановка, `busy`).
- Расхождения с планом (записать при архитектурном ревью): (а) кнопка «Создать машину…» в пустой библиотеке неактивна — создание bundle по умолчанию относится к S11-08; (б) при неожиданном выходе причина выхода главным текстом, но если прежнее уведомление само является причиной (failed, нарушение протокола, неудачный запуск), оно остаётся главным; (в) `LibraryModel.swift` — файл сверх плана; (г) тексты ошибок минимальные, полный каталог — S11-09.
- Находка: `HOME` не меняет `Catalog.standard()`; для изолированных запусков нужен `CFFIXED_USER_HOME`. Первый ручной запуск одного агента записал индекс в реальный `~/Library/Application Support/OpenDesktop` и был откатан; для smoke S11-14 нужна изоляция. Без индекса первый запуск сканирует `~/Documents/Open Desktop` и может вызвать запрос TCC. Вынести решение владельцу или в KNOWN_ISSUES.
- Проверки: `swift build` — exit 0; `swift test` — 165 тестов, 0 падений; `python3 tools/build/build_app_bundle.py` — exit 0, `codesign --verify --strict` — ok; `python3 tools/check_layout.py` — 45 ошибок (без изменений); реальные каталоги пользователя не созданы тестами.
- Не проверено: окно глазами и ручной чеклист `DESKTOP_APP.md` пункты 1–8; запуск test-only машины через UI; выход из приложения при запущенных машинах (S11-11); macOS 14.

### S11-07. C ABI создания и проверки RAW-образа

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено, **не принят** (архитектурного принятия нет).

- Файлы: `crates/od-host/src/storage_abi_v1.rs` (новый, C ABI и 18 тестов), `crates/od-host/src/lib.rs` (модуль только под macOS), `crates/od-host/src/storage_io.rs` (директорно-относительная форма создания `create_new_exclusive_at`; общая часть блокировки и размера вынесена в `lock_and_size`; поведение пути `Anchor::Path` не менялось), `include/open_desktop/storage_abi_v1.h`, `docs/specs/STORAGE_HOST_ABI_V1.md` (новый; создан вопреки запрету `docs/specs/*`, потому что файл назван в «Где» задачи), `tests/system/storage_abi_smoke.c`.
- ABI: `od_storage_abi_version`, `od_storage_create_raw_v1(dir_fd, name, name_len, capacity_bytes, flags, out_error)`, `od_storage_inspect_v1(dir_fd, name, name_len, out_info)`; статусы 0–11 закрытым списком; структуры по 32 байта с `struct_size`.
- Имя по VM_BUNDLE §3.3 (1–249 байт, UTF-8, без `.`/`..`/`/`/управляющих). Ёмкость кратна 512 (512 — гипотеза, кандидат LBA). Создание через `openat(O_CREAT|O_EXCL|O_NOFOLLOW)`; симлинк и любая существующая запись — `ALREADY_EXISTS`; откат только своего файла по совпадению identity; выход за каталог невозможен. Inspect не читает байты образа и не берёт lock.
- Durability создания не заявляется (ISSUE-055); статуса `unsupported_durability` в этом ABI нет по решению VM_BUNDLE §3.4.
- Проверки: `cargo test -p od-host --offline --lib -- storage_ raw_file` — 87 passed, 0 failed; `cargo clippy -p od-host --all-targets` по новым файлам — чисто; `rustfmt --check` по своим файлам — чисто; C smoke собран `clang -std=c11 -Wall -Wextra -Werror` и завершился с exit 0; `python3 tools/check_layout.py` — 45 ошибок (без изменений).
- Известные падения, не связанные с S11-07: полный `cargo test -p od-host --offline` падает (ISSUE-061: `MappingAlreadyLive`, `refusing to overwrite` в `work/`; на HEAD так же). `cargo fmt --all -- --check` падает в чужих файлах — ISSUE-062. Ошибки из-за отсутствующих `work/` в `check_layout` — прежние.
- Не проверено: реальная потеря питания и durability (ISSUE-055); сетевые и не-APFS тома; `fstatvfs` даёт 32-битный `f_bavail` (место насыщается около 16 ТиБ); открытие символьного устройства `O_RDONLY|O_NONBLOCK` до отказа по `fstat` (низкий риск); Linux-сборка модуля не проверялась (модуль только для macOS); вызов из VMHost и Swift — S11-04, S11-08.
- Открытые вопросы: маппинг статусов `od_storage_*` на коды control v2 — S11-03/S11-09; `INSUFFICIENT_SPACE` для smoke S11-08 — только заведомо большая ёмкость, инъекции через ABI нет (нужен test-only механизм в VMHost).

### S11-08. Мастер создания VM и команда `PREPARE_DISK`

**Текущий статус (2026-11-06):** S11-08 реализован полностью по частям: S11-08a (VMHost `PREPARE_DISK`, модель мастера, smoke) и S11-08b (окно мастера, исполнение плана §5.1, откат) — в рабочем дереве; ревью пройдено (Opus, блокеров нет); **не принят**; коммит — см. `git log`.

- S11-08b файлы: `apps/macos/Sources/App/CreateMachineWizard.swift` (лист мастера: имя → профиль → память → диск → сводка → выполнение; отмена до «Создать» ничего не создаёт; во время выполнения лист не закрывается), `apps/macos/Sources/App/CreateMachineExecutor.swift` (порядок §5.1: `mkdir` → подкаталоги → `lock` → для машины с диском VMHost без диска, `CONFIGURE` без `disks`, `PREPARE_DISK`, `STOP`, затем `config.plist` с `disks` последним; откат), `apps/macos/Tests/CreateMachineWizardTests.swift` (10 тестов, включая реальный `od-vmhost` с образом 1 MiB). Ошибка «конфигурация уже записана» — `creationConfigConflict`, откат не выполняется.
- Отступления от плана и спецификации: (1) откат удаляет только пустые каталоги, созданные этим запуском (`rmdir`), `lock` и образ не удаляются (VM_BUNDLE §5.1; задача говорила удалить каталог целиком); непустой каталог с `lock` остаётся и убирается пользователем (VM_BUNDLE §11 п. 13); (2) родительский каталог библиотеки создаётся с промежуточными каталогами и в откат не входит; (3) «Повторить» повторяет только запись `config.plist` при `busy` на шаге 4; (4) для машины без диска `lock` отпускается перед записью `config.plist` (writer берёт lock сам); безопасно по анализу ревьюера; (5) шаг ISO не добавлен (S11-08a).
- Известные ограничения, не исправлены в этом срезе: (а) пустые каталоги при откате удаляются без `lock` при отказе до его создания (`mkdir` неудачен) — данные не теряются, `rmdir` удаляет только пустое; (б) проверка «создан этим запуском» — по пути без `st_dev`/`st_ino`; в двухэкземплярном сценарии возможно удалить пустые каталоги чужой машины с тем же именем (содержимое не теряется); (в) нет таймаута на шаги VMHost: если VMHost завис, лист остаётся в «выполнении» — кнопки «Прервать» нет; (г) результат `library.open(bundle:)` не проверяется — при неудачном добавлении текст «Машина создана и добавлена» всё равно показан; (д) тестов «симлинк на месте каталога» и «чужой файл с этим именем» нет (код безопасен: `mkdir` даёт EEXIST); (е) ручной чеклист и вид листа в живом окне не проверены.
- Проверки: `swift build` — exit 0; `swift test` — 217 тестов, 0 падений; `check_layout` — 45 ошибок (без изменений).

### S11-09. Каталог ошибок и их показ

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено (Opus: блокеров нет, замечания исправлены), **не принят** (архитектурного принятия нет).

- Файлы: `docs/specs/USER_FACING_ERRORS.md` (новый; таблицы выведены из кода и проверены), `apps/macos/Sources/Shared/ErrorCatalog.swift` (ODShared: 25 кодов `ERROR`, 2 кода `ACK`, статусы и сигнал VMHost, сбои клиента и каталога, `error.unknown`), `apps/macos/Sources/App/ErrorPresentation.swift`, `apps/macos/Tests/ErrorCatalogTests.swift` (21 тест, включая сверку перечня кодов с таблицей §12.1 спецификации). Изменены `MachineController`, `LibraryModel`, `LibraryView`, `MachineRowView`, `ControlV2`, `ControlClientV2` (только `CaseIterable`), тесты контроллера.
- Исправлено по ревью: таблицы §5, §6, §8 спецификации имели съехавшие колонки (ключ в заголовке, без действия) — перестроены с колонкой «Ключ»; баннеры открытия и сохранения списка показывают «Что делать»; состояние `failed` по `STATE_CHANGED` больше не выдаётся за `ERROR` с кодом (подробности: «Состояние: failed (по событию STATE_CHANGED)»); текст `unsupported_durability` без «(этап 9)» и без привязки только к дискам; «Не получилось создать процесс машины» вместо тавтологии; «гостевой системы» единообразно; `lock_invalid` — «замените файл блокировки пустым обычным файлом».
- Расхождения с планом: `Localizable.strings` не создан (в SwiftPM-целях нет ресурсов), русские тексты — в `ErrorCatalog` с ключами; `vmhost.log` никто не пишет (владелец записи не назначен — решить); Request ID в отказах запросов приложению недоступен (нужна передача в завершение запроса `ControlClientV2`); мастер создания — S11-08, его ошибки пока не в каталоге; severity не используется в интерфейсе; слово «заново» допускается там, где это перезапуск или пересчёт, а не повтор небезопасной операции (тест запрещает «повтор», «снова», «ещё раз», «еще раз», «попробуйте», «позже»).
- Проверки: `swift build` — exit 0; `swift test` — 186 тестов, 0 падений; `python3 tools/check_layout.py` — 45 ошибок (без изменений); ссылки спецификации существуют; пробелы в конце строк — пусто; реальные каталоги пользователя не созданы.
- Не проверено: ручной чеклист UI (ни один диалог не показывался на экране); русская грамматика проверена ревьюером по тексту, без словаря.

### S11-10. Окно дисплея и ввод внутри приложения

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено (Opus: блокеров нет; меню и перепроверка закрытия исправлены), **не принят** (архитектурного принятия нет). Окно машины с живой машиной и интерактивное поведение глазами **не проверены**, скриншотов нет.

- Файлы: `apps/macos/Sources/App/MachineWindowController.swift`, `MachineWindowModel.swift` (чистая модель текстов и политики закрытия), `apps/macos/Tests/MachineWindowTests.swift` (31 тест + 1 с настоящим `od-vmhost`); изменены `apps/Package.swift` (библиотечная цель `ODDisplay` из общих файлов `DisplayWindow`, `MetalPresenter`, `InputChannel`, используется приложением и viewer), `Sources/DisplayWindow.swift`, `Sources/MetalPresenter.swift` (overlay, `drawFrame`, захват ввода — поведение viewer сохранено), `Sources/display_viewer_main.swift` (`import ODDisplay`), `App/MachineController.swift` (запуск с `--display-fd 5` и `--input-fd 4` только при открытом окне), `App/LibraryModel.swift`, `App/LibraryView.swift` (меню «Вид»), `App/OpenDesktopApp.swift`.
- Поведение: окно открывается на `running`; до `ANNOUNCE` — «Нет сигнала» (лимит 10 с); «Пауза», «Остановлено», «Остановлено: сбой машины», «Несовместимая версия» — текстом поверх, кадр не замораживается; закрытие при `running`/`paused` — вопрос и `STOP`; захват ввода — ⌃⌥⌘G или меню «Машина», только при `running`, ключевом окне и канале в READY; захват снимается при паузе, остановке, потере фокуса, сворачивании и закрытии.
- Решения по захвату и конфликты — `apps/macos/README.md`; ⌃⌥⌘G сверено только со страницей Apple 102650, конфликты с пользовательскими настройками, Accessibility и сторонними программами не проверены.
- Исправлено по ревью: обновление меню и заголовка только при смене состояния читателя (раньше — на каждом кадре); решение о закрытии перепроверяется после вопроса.
- Проверки: `swift build` — exit 0; `swift test` — 249 тестов, 0 падений; `display_viewer_dump_smoke.py` — exit 0; `display_viewer_scaled_smoke.py` — exit 0; `python3 tools/check_layout.py` — 45 ошибок; `build_app_bundle.py` — exit 0; запуск приложения из bundle с `CFFIXED_USER_HOME` — процесс жив, одно окно 900×450 в `CGWindowList`.
- Отступления от плана: (1) `DisplayWindow`, `MetalPresenter`, `InputChannel` лежат в цели viewer и приложению недоступны — вынесены в библиотечную цель `ODDisplay`, без переноса файлов; (2) правки `DisplayWindow` и `MetalPresenter` нужны приложению (overlay, режимы захвата), для viewer поведение прежнее; (3) чистая модель в отдельном файле `MachineWindowModel.swift` для тестов без AppKit; (4) состояния окна «Запуск…» и «Останавливается…» сверх плана.
- Не проверено глазами: оверлеи, заголовок и подзаголовок; порядок меню «Вид» и «Машина» — возможен дубль с системным «View» (гипотеза), и совпадение ⌃⌘F с системным «Enter Full Screen» (записано в README); полноэкранный режим, скрытие курсора и захват с клавиатуры; вопрос при закрытии; несколько мониторов и @2x.
- Замечания, не исправленные в срезе: проводка `handleKey`/`setInputCaptured` тестами не покрыта (проверена кодом); `package`-модификаторы без внешних пользователей; порядок `display.update` и `setOverlay` (возможен один лишний кадр с фоном при переходе); мёртвый параметр `detail` в `MachineWindowModel.overlay`; Cmd-Q закрывает окна без `STOP` — задача S11-11.
- Остатки: второй экран двухэкранного профиля (S10-15); настройка комбинации захвата; отвязка курсора от мыши; повторное открытие окна для уже идущего запуска.

### S11-11. Выход из приложения, сбой VMHost и повторный запуск

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено (Opus: блокеров нет; замечания по выходу при `shuttingDown` и по smoke исправлены), **не принят** (архитектурного принятия нет). Лист выхода, кнопки и оверлей глазами не проверены; Cmd-Q с работающей машиной и `ps` не прогонялись.

- Файлы: `apps/macos/Sources/App/AppLifecycle.swift` (`QuitRules` — чистая функция решения; `QuitTeardown` — ожидание выхода; `AppLifecycle` — `applicationShouldTerminate` и лист подтверждения), `apps/macos/Tests/AppLifecycleTests.swift` (13 тестов), `tests/system/desktop_vmhost_crash_smoke.py` (smoke без UI: SIGSEGV и SIGKILL в `running` → сигнал, без `STATE_CHANGED → stopped`, lock свободен; `STOP` в `paused` → `stopped`, EOF, exit 0; второй VMHost на тот же bundle → `ERROR busy`, статус 3). Изменены `ControlClientV2.swift` (`processID`), `MachineController.swift` (сбой → `stopped` с причиной, «Запустить снова», «Показать лог», `busy` с pid и «Завершить процесс» только при известном pid), `MachineRowView.swift`, `LibraryModel.swift`, `OpenDesktopApp.swift`, `MachineWindowModel.swift`, `MachineWindowController.swift`, тесты контроллера.
- Выход: при `running`/`paused` — лист с именами и «Остановить и выйти» / «Отмена»; при `shuttingDown` — тоже лист (исправлено по ревью: гость не получает `STOP` без вопроса); машины в `starting` и без `STOP`-возможности завершаются молча; `STOP` всем, ожидание 10 с, затем SIGTERM/SIGKILL по правилам клиента, 3 с запаса.
- Проверки: `swift build` — exit 0; `swift test` — 278 тестов, 0 падений (включая тест с настоящим `od-vmhost`: `kill(SIGSEGV)` → `stopped` с причиной, lock свободен, повторный запуск); `desktop_vmhost_crash_smoke.py` — exit 0, `ok` (5 запусков подряд в ходе работы агента; один повторно при ревью); `vmhost_control_v2_lifecycle_smoke.py` — exit 0; `build_app_bundle.py` — exit 0; `check_layout` — 45 ошибок; `ls` реальных каталогов пользователя — не созданы.
- Отступления от плана и открытые решения: (1) кнопки «Запустить снова» и «Показать лог» стоят в строке библиотеки, в окне машины — только текст; (2) заголовок сигнала совпадает с записью каталога S11-09, текст «Процесс машины завершился: процесс машины аварийно завершился» тавтологичен — подлежит правке; (3) ветка «занята процессом (pid N)» с кнопкой «Завершить процесс» при `start()` в `stopped`/`failed` недостижима из UI (кнопка «Запустить» неактивна при живой сессии) — решение владельца: показывать «Завершить процесс» в строке или разрешить `start` при живой сессии; (4) pid при `busy` от VMHost — pid держателя неизвестен протоколу: текст идёт без pid и без кнопки; (5) SIGTERM без `STOP`, если у машины запрос в полёте (`PAUSE`/`RESUME`), — вопрос обещает «команду остановки», а процесс получает SIGTERM сразу: можно дождаться ответа на запрос; (6) при отказанном при выходе `STOP` ждём 10 с впустую — можно сразу `forceTerminate`; (7) `static var machines` в `AppLifecycle` — глобальное состояние, задаётся в `App.init`; (8) smoke проверяет протокол напрямую, а не через `ControlClientV2`/контроллер (путь через контроллер — только unit-тест с настоящим VMHost); (9) Cmd-Q закрывает окна без `STOP` вне вопроса — учтено выше, сессия мастера создания при выходе не обрабатывается.
- Не проверено глазами: лист выхода, кнопки «Запустить снова», «Показать лог», «Завершить процесс» в строке, оверлей; Cmd-Q с работающей машиной и `ps` после выхода.

### S11-12. Сон/пробуждение Mac, смена дисплеев, нехватка места и памяти

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено (Opus: блокеров нет; замечания по паузе по месту во время сна и завершению таймера исправлены), **не принят** (архитектурного принятия нет). Настоящий сон, смена экранов и давление памяти **не проверялись**; ручная процедура — `docs/testing/DESKTOP_APP.md`, раздел 6.

- Файлы: `apps/macos/Sources/App/HostConditions.swift` (сон и пробуждение, смена экранов, проверка места, давление памяти, `WindowPlacement`, `--simulate-host-event`), `apps/macos/Tests/HostConditionsTests.swift` (36 тестов); изменены `MachineController.swift` (`pause(reason:)`, `resume(reason:)`, системная пауза, проверка места перед START), `CreateMachineExecutor.swift` (проверка места перед PREPARE_DISK), `ErrorCatalog.swift` (`HostConditionIssue`, 7 записей `host.*`), `ErrorPresentation.swift`, `LibraryModel.swift`, `LibraryView.swift`, `OpenDesktopApp.swift`, `AppOptions.swift`, `MachineWindowController.swift`, `DisplayWindow.swift`, тесты контроллера и окна; `docs/specs/USER_FACING_ERRORS.md` — раздел «Состояния хоста (S11-12)» (записи добавлены при сверке: агенту это было запрещено).
- Поведение: пауза перед сном только для `running` с `reason = system_sleep`; RESUME после пробуждения — только для системной паузы; пользовательская пауза и пауза по месту не снимаются; если пробуждение пришло раньше ответа на PAUSE, RESUME уходит, когда пауза дошла; отказ PAUSE сохраняется до пробуждения и показывается; START не уходит при жёстком пороге места (5 GiB предупреждение, 1 GiB жёсткий — гипотеза); PREPARE_DISK при жёстком пороге не отправляется; проверка места в running — раз в 30 с; смена экранов переносит окно на основной экран, если оно вне видимой области; баннер давления памяти не останавливает машины.
- Исправлено по ревью: пауза по месту, отправленная до сна, не помечается как «пауза не отправлена» (отдельный флаг, сбрасывается по ответу); таймер и источник давления памяти отменяются при завершении приложения (`willTerminate`); комментарии приведены в соответствие коду.
- Проверки: `swift build` — exit 0; `swift test` — 318 тестов, 0 падений; `desktop_vmhost_crash_smoke.py` — exit 0; `check_layout` — 45 ошибок; `build_app_bundle.py` — exit 0 (сборка до последней правки: повторно не проверялась); реальные каталоги пользователя не созданы.
- Отступления и открытые пункты: (1) отказ PAUSE показывается строкой машины, в лог не пишется (в приложении нет логгера; план п. 1 требует «в лог и уведомление»); (2) машина в `starting` в момент сна пропускается молча и может стать `running` без паузы — не записано в README как ограничение, открыто; (3) `--simulate-host-event low-space` и `sleep` без `wake` действуют на весь запуск и на все машины — описано в README как test-only; (4) RESUME уходит и тогда, когда сон был прерван раньше ответа на PAUSE — сверх плана, чтобы машина не застряла приостановленной системой; (5) Context7: предел времени обработчика `willSleep` и гарантия, что асинхронный PAUSE успеет до сна, не найдены — PAUSE отправляется и не ждётся; (6) пороги места и интервал 30 с — гипотезы без замера; место проверяется по `volumeAvailableCapacityForImportantUsageKey`, ABI проверяет `f_bavail*f_frsize` — расхождение возможно; (7) память — `DispatchSource.makeMemoryPressureSource`, подтверждено Context7.
- Не проверено: сон (крышка, `pmset sleepnow` вручную), пробуждение и перерисовка окна; отключение внешнего монитора; давление памяти и вид баннера; задержка паузы между `willSleep` и сном.

### S11-13. CLI `odctl` на том же контракте

**Текущий статус (2026-11-06):** реализован в рабочем дереве, ревью пройдено (Opus: блокеров нет; пункты «исправить до приёмки» исправлены), **не принят** (архитектурного принятия нет). Ручной вывод на живом Mac по командам CLI не выполнялся, только smoke во временных каталогах.

- Файлы: `apps/cli/Sources/odctl/` (`main`, `Arguments`, `Output`, `Library`, `QueryCommands` — list, inspect, delete; `CreateCommand`; `RunCommand`; `HostSession` — блокирующая обёртка над `ControlClientV2`; `Script`), `apps/cli/README.md`, `docs/specs/CLI.md` (новый), `tests/system/odctl_lifecycle_smoke.py`, `apps/macos/Sources/Shared/OdctlRules.swift` (новый: разбор сценария, имя каталога, правило удаления), `apps/macos/Tests/OdctlRulesTests.swift` (22 теста), `ControlClientV2.closeConnection()`; `apps/Package.swift` (цель `odctl`, зависит только от `ODShared`); `docs/PROJECT_LAYOUT.md` (строка `apps/cli/Sources/odctl` в перечне).
- Поведение: команды `list`, `inspect`, `delete`, `create`, `run` без демона и endpoint; коды 0–4; `"schema": 1` в JSON; машина с диском без `start` — `run` завершается с кодом 0 (соединение закрывается, VMHost видит EOF в `stopped`); `start` или `stop` для машины с диском — код 4 до запуска VMHost; SIGINT/SIGTERM — последняя строка `result` и код 1; `delete --files` держит lock до конца удаления; при нечитаемой конфигурации с `disks/` — код 4, файлы целы.
- Отступления от плана и решения (записаны в `docs/specs/CLI.md` §7): (1) логика создания повторена в CLI, а не вынесена в ODShared (`CreateMachineExecutor` тянет `ErrorPresentation` и `Combine`); (2) правило удаления вынесено в ODShared (`BundleRemoval`) и используется только CLI — приложение (`LibraryModel`) пока на своей копии, перевод — следующий срез; (3) явный `stop` для машины с диском даёт код 4 до запуска — сверх задания; (4) в CLI нет проверки свободного места и отката при ошибке создания; (5) при занятом имени каталога `create` — ошибка, без числового суффикса VM_BUNDLE §2 п. 4; (6) SIGINT/SIGTERM во время `create --disk-gib` не обрабатываются; (7) `ControlClientV2.closeConnection()` и `OdctlRules.swift` добавлены в ODShared — это выход за формулировку «Где» задачи, отмечено здесь.
- Проверки: `swift build` — exit 0; `swift test` — 340 тестов, 0 падений; `odctl_lifecycle_smoke.py` — exit 0, `ok` (3 запуска подряд, и один при сверке); `desktop_vmhost_crash_smoke.py` — exit 0; `check_layout` — 45 ошибок; реальные каталоги пользователя не созданы; `--help` и `docs/specs/CLI.md` совпадают по командам.
- Не проверено: ручной запуск с настоящими `~/Documents` и `~/Library` (намеренно не делался); занятость машины именно приложением (проверена только занятость другим `odctl`, механизм один — `flock`); потеря записи индекса при одновременной записи приложением и CLI (последний выигрывает); `create --disk-gib` на нехватке места и больше 1 GiB.

### S11-14. Сквозной срез оболочки и измерения

**Текущий статус (2026-11-06):** реализован в рабочем дереве, **не принят** (ревью подпрограммой не проводилось). Сценарий и измерения test-only, без гостя.

**Что работает.** `python3 tests/system/desktop_lifecycle_smoke.py` собирает bundle (`build_app_bundle.py`, release) и проходит 5 повторов в одном запуске: `odctl create`, `odctl run --json-events` (start, wait running, pause, wait paused, resume, wait running, stop; переходы stopped→running→paused→running→stopped, exit 0), запуск `OpenDesktop --open <bundle> --vmhost <helper>` на 4 с (процесс жив, stderr пуст), `odctl delete --files`. Каждый повтор во временных библиотеке и home; реальные каталоги пользователя не создаются. Проверено 5 раз подряд отдельными запусками (exit 0) и повторно при сверке.

**Исправлено при сверке.** Первая версия проверки окна искала только окна «на экране» (`CGWindowListCopyWindowInfo` с опцией on-screen) и падала: приложение, запущенное не активированным, показывает окно в списке всех окон (900×450, владелец «Open Desktop»), но `kCGWindowIsOnscreen` пустой. Проверка переведена на все окна: окно процесса должно существовать; состояние «на экране» печатается отдельно (в текущих прогонах 0). Окно глазами и его видимость на экране не проверялись.

**Измерения** (`tools/bench/desktop_lifecycle_bench.py`; данные — `docs/reports/data/stage11/s11-14-*-2026-11-06.json`, каждый не больше 7.2 KiB).
- Условия: Mac15,11 (M3 Max), macOS 26.7.1 (25G241), bundle в release, odctl в debug, ревизия `831fad8` (отслеживаемые файлы чистые), питание от сети, load average 6.0 / 5.5 / 4.2, профиль `display-test-pattern`, 256 MiB.
- 5 учтённых повторов на метрику, прогрев отброшен и записан отдельно (`counted: false`); трактовка «прогрев входит в пять или нет» — решение владельца.
- START → running: median 2.04 мс (min 1.92, max 2.72). Вторая серия START → ACK: median 2.05 мс.
- PAUSE → paused: median 0.109 мс (0.069 … 0.121). Вторая серия PAUSE → ACK: median 0.121 мс.
- PREPARE_DISK 64 GiB (запрос → ACK): median 0.135 мс (0.129 … 0.175). Вся команда `odctl create --disk-gib 64`: median 12.3 мс (11.9 … 12.5). Логический размер 68719476736 байт, выделено 0 байт (sparse). Свободного места было около 66.8 GiB, отказа `insufficient_space` не было. Образ удалялся после каждого прогона.
- RSS od-vmhost: 6208 KiB после READY; около 43.4 MiB через 1 с после running и после paused. RSS приложения: median 91488 KiB (91312 … 92160), через 1 и 4 с после запуска.
- Сравнения вариантов нет: числа — база для порогов S11-12 и S11-61, а не заявление о скорости. Данные test-only, на реальную машину не переносятся.

**Не проверено.** Сон Mac и реальное поведение ОС (`--simulate-host-event` не использовался), смена экранов, нехватка места и памяти; окно глазами и скриншоты; Finder и `open`; macOS 14; контрольный M1 Pro; гостевой уровень; RSS при длительной работе (S11-61). Debug-сборка VMHost не измерялась (бенчмарк — release). Вывод «приложение готово» не делается: нет ОС, агента, сети; машины с дисками не запускаются до S9-06.

### S11-20. ADR-0007 и протокол гостевого агента

_Не начата. Раздел заполняется вместе с задачей._

### S11-21. Устройство канала агента в `od-machine` и ABI

_Не начата. Раздел заполняется вместе с задачей._

### S11-22. Сервис агента в VMHost: handshake, время, shutdown

_Не начата. Раздел заполняется вместе с задачей._

### S11-23. Freestanding probe агента — сквозной срез без ОС

_Не начата. Раздел заполняется вместе с задачей._

### S11-24. Буфер обмена (текст): host-часть

_Не начата. Раздел заполняется вместе с задачей._

### S11-25. Drag-and-drop файлов: контракт и host-часть

_Не начата. Раздел заполняется вместе с задачей._

### S11-26. Общие папки: контракт и host-сервис с защитой корня

_Не начата. Раздел заполняется вместе с задачей._

### S11-27. Гостевой агент Windows

_Не начата. Раздел заполняется вместе с задачей._

### S11-28. Гостевой агент Linux

_Не начата. Раздел заполняется вместе с задачей._

### S11-29. Гостевые драйверы общих папок и сквозной срез

_Не начата. Раздел заполняется вместе с задачей._

### S11-30. ADR-0008 и контракт сети

_Не начата. Раздел заполняется вместе с задачей._

### S11-31a. Trait backend, очереди, разбор L2/L3 и устойчивость к мусору

_Не начата. Раздел заполняется вместе с задачей._

### S11-31b. DHCP и DNS-пересылка

_Не начата. Раздел заполняется вместе с задачей._

### S11-31c. UDP и ICMP echo

_Не начата. Раздел заполняется вместе с задачей._

### S11-31d. TCP

_Не начата. Раздел заполняется вместе с задачей._

### S11-31e. Измерение NAT на loopback

_Не начата. Раздел заполняется вместе с задачей._

### S11-32. Модель NIC, совместимая с Intel 82540EM

_Не начата. Раздел заполняется вместе с задачей._

### S11-33. Сквозной сетевой срез с Linux

_Не начата. Раздел заполняется вместе с задачей._

### S11-34. Измерения сети с гостем

_Не начата. Раздел заполняется вместе с задачей._

### S11-40. Класс аудиоустройства, правка платы и контракт

_Не начата. Раздел заполняется вместе с задачей._

### S11-41. Host backend звука с offline-проверкой

_Не начата. Раздел заполняется вместе с задачей._

### S11-42. Модель аудиоустройства

_Не начата. Раздел заполняется вместе с задачей._

### S11-43. Сквозной аудиосрез

_Не начата. Раздел заполняется вместе с задачей._

### S11-50. Scope интеграции гостевых окон

_Не начата. Раздел заполняется вместе с задачей._

### S11-51. Scope USB-классов и печати

_Не начата. Раздел заполняется вместе с задачей._

### S11-60. Полный пользовательский сценарий

_Не начата. Раздел заполняется вместе с задачей._

### S11-61. Длительный прогон приложения

**Статус:** реализован, два часовых прогона выполнены (2026-11-06), **не принят**. Зависаний, переходов вне таблицы, нераспознанных сообщений, принудительных завершений VMHost и сирот нет. Критерий роста RSS приложения по схеме «начало и конец» не может служить приёмкой: на этом хосте он даёт разброс, сравнимый с порогом (первый прогон +22.21 %, второй −23.5 % при том же смысле замера). Вывод об утечке или её отсутствии не делается.

**Команда:** `python3 tools/bench/desktop_soak.py --duration-minutes 60 --configuration release` (нужны собранные bundle и `odctl`). Скрипт возвращает 1, если нарушен хоть один критерий; JSON пишется в любом случае.

**Что делал прогон.** Профиль `display-test-pattern`, 256 MiB, test-only, без гостя и без диска. Три машины создаются через `odctl create` во временной библиотеке (`HOME` и `CFFIXED_USER_HOME` временные): две soak-машины A и B и третья `soak-app` только для приложения. Каждую soak-машину держит сессия control v2; `odctl run` не использовался, потому что его сценарий фиксирован и процесс заканчивается вместе со сценарием. Каждые 5 минут — PAUSE, 2 с, RESUME по очереди на A и B. Каждые 15 минут — STOP одной машины (код 0), затем новый `od-vmhost`, CONFIGURE, START. Приложение одно долгоживущее на весь прогон (`--open soak-app --simulate-host-event memory`). События sleep, wake, screens, low-space, memory поочерёдно запускают короткоживущие экземпляры приложения по 3 с. Это симуляция обработчиков через `--simulate-host-event`, не сон Mac, не реальные экраны, место и память. Скрипт запущен под `caffeinate -i`.

**Проверочные раунды.** Для окон «первые и последние 10 минут» расписания дают мало образцов, поэтому добавлены проверочные раунды (`source = probe`): 20 PAUSE/RESUME и 6 STOP/START. Они посчитаны отдельно от расписания и записаны в JSON.

**Прогоны.**

| Прогон | Скрипт | Итог по критериям | RSS приложения, начало → конец | Ряд замеров приложения (61 точка) | Макс. рост VMHost за поколение |
|---|---|---|---|---|---|
| run1 | до правок ревью | «RSS приложения» не пройдено: +22.21 % | 97968 → 119728 KiB | 67904–139920 KiB, медиана 122064 | 2.22 % |
| run2 | после правок ревью | все критерии выполнено, RSS приложения −23.5 % | 98320 → 75216 KiB | 74800–98576 KiB, медиана 75232 | 0.44 % |

Ряды замеров взяты из лога прогона; лог лежит во временном каталоге и в репозиторий не вошёл. Файлы: `docs/reports/data/stage11/s11-61-soak-2026-11-06-run1-prefix.json` (run1) и `docs/reports/data/stage11/s11-61-soak-2026-11-06.json` (run2).

**Результаты run2 по критериям.** Зависаний 0 (204 команды, 204 ответа; 120 проверок QUERY совпали с событиями). Переходов вне таблицы 0, событий без команды 0. Нераспознанных кадров 0, строк stderr у VMHost и приложения 0. VMHost с кодом не 0: 0; принудительных завершений (SIGKILL): 0; ошибок при остановке: 0; оставшихся `od-vmhost`: нет. Реальные каталоги пользователя не появились. Циклы: pause/resume 11 по расписанию и 20 проверочных; stop/start 3 и 6; симулированные события sleep 3, wake 3, screens 2, low-space 2, memory 3.

**Задержки, run2 (медианы, мс; первые → последние 10 минут).** PAUSE → paused 0.136 → 0.085 (n 11 → 12); RESUME → running 0.137 → 0.134 (n 11 → 12); STOP → stopped 0.074 → 0.113 (n 3 → 3); START → running 2.573 → 2.293 (n 5 → 3, START входит в «extra»). Порога для задержек нет; отношения записаны как наблюдение, а не как критерий. Вывода «быстрее» или «медленнее» по ним делать нельзя. Задержки run1 (файл run1-prefix): PAUSE 0.133 → 0.105, RESUME 0.164 → 0.088, STOP 0.117 → 0.143, START 2.111 → 2.032 мс.

**Почему RSS по `ps` не годится как мера утечки.**
- Ряд RSS приложения за один прогон колеблется примерно от 68 до 140 MiB (run1) и от 75 до 99 MiB (run2). Значения «начало» и «конец» попадают в разные фазы этого колебания, поэтому знак и величина изменения определяются моментом замера.
- RSS VMHost по `ps` у «спящих» процессов падает примерно с 43 MiB до 5.8 MiB за 5–20 минут жизни (вероятно, сжатие страниц macOS; гипотеза не проверена). Поэтому «рост VMHost ≤ 10 %» формально выполняется и при отрицательных значениях, что не говорит об освобождении памяти процессом.
- Нужна другая мера: `phys_footprint` (`footprint -p <pid>` или `top -stats mem`), со множеством замеров внутри прогона, а не только в двух точках.

**Ревью скрипта.** Раунд 1 (Opus, medium): четыре major — принудительные завершения и ошибки остановки не влияли на вердикт, проверка сирот шла после очистки, «не оценено» не мешало `passed = true`, SIGTERM не запускал `finally`. Все четыре исправлены. Проверки исправлений: короткий прогон на 2 минуты проходит по всем критериям; SIGTERM в середине прогона даёт код 1, процессов и временного каталога не остаётся. Независимого повторного ревью исправлений не было; оно входит в S11-62.

**Не исправлено (minor из ревью).** `percent_growth` при нулевом первом значении даёт 0; STATE_CHANGED с чужим ID считается нераспознанным, а не событием без команды; таймаут `recv` может вдвое превысить заявленный; холодный START попадает в первое окно задержек; мёртвый код `force_wait` и `Machine.last_command`.

**Не проверено.** Настоящий сон Mac и пробуждение; реальная смена дисплеев; настоящее давление памяти и нехватка места; окно машины и окно приложения глазами; PAUSE/RESUME на работающей машине приложения (приложение без окна не запускает машину, поэтому `soak-app` всё время `stopped`, а обработчики sleep/wake/low-space проверены без машины приложения); гость (ОС, агент, сеть). Условия не контролировались: обычная интерактивная сессия; свободное место 63.5 → 58.4 GiB (run1).

**Решения владельца.** (1) Заменить метрику RSS на `phys_footprint` с несколькими замерами внутри прогона и провести новый прогон; (2) решить, какой порог принимать, и подтвердить, что гипотеза 10 % не является критерием до замера; (3) решить, нужен ли приложению окно или отдельный тестовый режим, чтобы проверять PAUSE/RESUME при симулированных событиях на работающей машине приложения.


### S11-62. Промежуточное ревью и приёмка gates G1–G3

_Не начата. Раздел заполняется вместе с задачей._

### S11-70. Независимое итоговое ревью host-частей

_Не начата. Раздел заполняется вместе с задачей._

### S11-71. Документация и приёмка этапа в достигнутых границах

_Не начата. Раздел заполняется вместе с задачей._

## 4. Открытые ограничения

- Этапы 5, 7 и 8 не начаты: загружающейся гостевой ОС нет; все задачи с гостем blocked, оболочка проверяется только на test-only профилях дисплея (1 и 4) без гостя.
- Durability (ISSUE-055, S9-06): до закрытия mutating операции VM и durable operation record недоступны, ответ — `unsupported_durability`; durability создания bundle, диска и записи `config.plist` не заявляется.
- Сессия «CPU + устройства» (S10-09) и PCI (S4-30) не выполнены: probe агента, модель NIC и PCI-аудио blocked.
- Открытые вопросы для S11-13 (`CLI.md`): как `odctl` находит `od-vmhost`; как `odctl run` без UI получает ISO и диски вне bundle (пути из аргументов или сценария: кто открывает файл, как путь превращается в дескриптор для `ATTACH_RESOURCE`, допускает ли сценарий пути) — правило ADR-0006 «пути только через `NSOpenPanel`» относится к приложению.
- Подтверждения владельца ожидают: решение (б) ADR-0006 (App Sandbox и доступ к файлам), транспорт агента (S11-20), стек NAT (S11-30), включение звука (S11-40).
