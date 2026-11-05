# Этап 9. Надёжность данных и управление состоянием — пошаговый план реализации

Статус: **план подготовлен, реализация этапа не подтверждена**. Исходное состояние проверено 2026-11-05. Все задачи S9 ниже открыты; наличие документа, каркаса или успешной сборки не закрывает задачу.

Нормативный объём — [этап 9 дорожной карты](../ROADMAP.md#этап-9-надёжность-данных-и-управление-состоянием). Обязательные границы задают [архитектура](../ARCHITECTURE.md), [карта проекта](../PROJECT_LAYOUT.md), [виртуальная плата](../specs/VIRTUAL_BOARD.md) и [политика зависимостей](../policies/DEPENDENCIES.md). Это план выполнения, а не второй формат диска или новая архитектура.

## 1. Результат этапа и первый срез

Собственный storage сохраняет подтверждённые данные при предусмотренных сбоях, проверяет целостность, восстанавливается воспроизводимо и позволяет экспортировать данные. Снимок всей VM объединяет CPU, RAM, устройства, время, I/O, диски, UEFI variables и vTPM в одно согласованное состояние. Пользователь различает pause, suspend, shutdown и disk snapshot; последствия restore и clone определены явно.

Работу вести двумя последовательностями:

1. **Независимый storage:** RAW с одним writer → явный durable flush → небольшой собственный CoW → recovery после сбоя → неизменяемые snapshots → check/export. Этот путь можно реализовывать до работающей Windows.
2. **Состояние машины:** готовые save/restore контракты компонентов → общий barrier CPU/I/O → сохранение всех участников под одним manifest → проверка и staged restore → реальные Linux/Windows сценарии. Этот путь требует реализованных компонентов машины и не заменяется fixture snapshot.

Первый вертикальный срез: создать маленький одноразовый RAW, записать нумерованные blocks, подтвердить flush, аварийно завершить writer, открыть заново и проверить данные. Следующий срез повторяет опыт с собственным CoW и независимой моделью ожидаемого результата. RAM suspend, UI и vTPM не добавлять до готовности основания.

Вне первой версии: QCOW2/VMDK parsers, distributed storage, network filesystem как неподтверждённый durable backend, live migration, background live snapshot без остановки, дедупликация, сложная compression, новое шифрование всего диска и произвольный импорт чужих VM. Защита ключей vTPM и чувствительного сохранённого состояния при этом обязательна для заявленного security profile. Штатные криптографические примитивы допустимы; протоколы и управление состоянием остаются собственными.

## 2. Фактическая отправная точка

| Область | Факт на 2026-11-05 | Следствие |
|---|---|---|
| `crates/od-storage/` | README и `src/lib.rs` прямо обозначают scaffold без функциональности | RAW, CoW, recovery и snapshot container предстоит реализовать |
| `tools/image-tool/src/` | Пока `.gitkeep` | Наличие каталога не означает существования CLI или parser |
| CPU/JIT/MMU | Этапы 2–4 имеют принятые ограниченные срезы; этап 4 целиком открыт | Нельзя объявлять полный CPU/RAM/device checkpoint или рабочую VM по этим срезам |
| Планирование этапов | В `docs/plans/` есть планы 0–4; планов 5–8 на момент проверки нет | Зависимости брать из roadmap и фактических контрактов; не придумывать выполненные S5/S7/S8 задачи |
| Архитектура | RAW сначала; затем versioned CoW; single writer; CPU/I/O barrier; JIT cache не обязателен в snapshot | Сохранять этот порядок и существующих владельцев |
| Firmware/TPM | UEFI variables и vTPM — отдельные persistent artifacts, не часть обычной RAM | Для полного checkpoint нужны реальные codecs и политика восстановления этих компонентов |

Ближайшие источники статуса: [план этапа 4](STAGE_4.md), [реестр проблем](../KNOWN_ISSUES.md), [матрица поддержки](../specs/SUPPORT_MATRIX.md). Перед каждой реализацией перечитать актуальный статус: таблица выше — снимок состояния, а не вечный запрет на использование будущих возможностей.

Незакрытые prerequisites отмечать на уровне конкретного сценария. Отсутствие Windows/vTPM не блокирует offline CoW; отсутствие save/restore реального vTPM блокирует checkpoint профиля с vTPM. Тестовая машина без этого устройства допустима как отдельный ограниченный профиль, но не как приёмка полной Windows VM.

## 3. Правила исполнителя и размещение

1. Прочитать корневой `AGENTS.md`, этот план, карту проекта и вложенные инструкции по пути каждого изменения. Проверить пользовательский diff; не присоединять чужие изменения к своей задаче.
2. Взять одну задачу с выполненными необходимыми зависимостями. До кода написать: вход, наблюдаемый результат, failure outcomes, ownership и ограничения. Не создавать заранее пустые модули.
3. Discovery начинать с Graphify. Внешнюю документацию SDK, API и библиотек получать исключительно через Context7 согласно правилам проекта. Если точного источника нет, зафиксировать отсутствие подтверждения и оставить соответствующую платформенную гарантию открытой до получения проверяемого источника. Не угадывать durability файлового API по названию функции.
4. Роли по `AGENTS.md`: код — `gpt-6-luna` / `high`; архитектурные решения — `gpt-6-astra` / `high`; независимое ревью — `gpt-6.1-sol` / `high`. Разделять независимые задачи, не править один контракт одновременно несколькими авторами.
5. После каждого среза выполнить необходимые проверки и отдельное ревью diff/контрактов. Исправить замечания; записать результат. Итоговый S9-43 не заменяет ревью промежуточных задач.
6. Для Rust — `cargo fmt --all -- --check`, сборка затронутых компонентов и поведенческие tests; итоговый host check — `cargo check --workspace --all-targets --offline`. При изменении структуры — `python3 tools/check_layout.py`. Firmware/guest отдельно собирать для их target: host check их не проверяет.
7. После кода обновить Graphify. Отчёт фиксирует выполненные проверки, неисполненные условия и scope. Новые подтверждённые дефекты и непроверенные гипотезы различимо записывать в `KNOWN_ISSUES.md`; будущую функциональность целиком туда не переносить.
8. Не подключать готовый disk/VM snapshot engine вместо собственной реализации. General-purpose serialization/checksum/crypto/testing libraries допустимы по политике зависимостей, с назначением, версией, лицензией и features. Rust layout, native pointers и структура `serde` по умолчанию не задают долговечный формат.

При реализации создать документы `docs/specs/STORAGE_FORMAT.md`, `docs/specs/VM_STATE.md`, `docs/testing/STORAGE_RECOVERY.md`, `docs/reports/STAGE_9_REPORT.md`. Первые два — контракт persistent форматов и состояния; методика описывает испытания, отчёт — факты. Межпроцессные/guest wire formats сохранять нормативно в `protocols/`, а docs должны ссылаться на них. Не создавать параллельный ABI в этих спецификациях.

| Что меняется | Владелец |
|---|---|
| RAW/CoW, parser, disk metadata, snapshot container, recovery/format migration | `crates/od-storage/src/` |
| Barrier vCPU/I/O, epoch, RAM и device state, orchestration checkpoint/restore | `crates/od-machine/src/` |
| Версионированный архитектурный CPU state и его валидация | `crates/od-isa/src/` |
| Сброс/воссоздание transient JIT state | `crates/od-jit/src/` |
| Host file durability/locking/resources через узкий внедряемый interface; C ABI glue | `crates/od-host/src/`, `include/open_desktop/` |
| Жизненный цикл VMHost, связи с UI | `apps/vmhost/Sources/`, `apps/macos/Sources/`, `protocols/control/` |
| Семантика UEFI variables; guest-target реализация firmware | `firmware/uefi/src/` |
| Собственный TPM device/protocol state; persistent bytes через storage, host защита ключей | `crates/od-machine/src/`, `od-storage`, `od-host` по их границам |
| Guest quiesce/thaw и guest OS integration | `guest/linux/agent/src/`, `guest/windows/agent/src/`, `protocols/guest/` |
| CLI поверх storage, без второго parser/recovery алгоритма | `tools/image-tool/src/` |
| Orchestration, benchmark, fault harness и малые собственные fixtures | `tools/runner/src/`, `tools/bench/`, `tests/system/`, `tests/fixtures/guest/` |

`od-storage` не управляет vCPU и не импортирует `od-host`; host I/O приходит через узкий interface. `od-machine` согласует состояние, `od-storage` записывает контейнер. Firmware не включать в host workspace ради сериализации. UI-индекс VM восстанавливаемый; он не единственная копия критической конфигурации и metadata.

## 4. Контракты, которые нельзя упростить ради прохождения теста

### 4.1. Подтверждение и сохранность

- Различать ordinary write completion, visibility следующему read и **durable acknowledgement**. Задать точную границу guest flush/barrier/FUA для поддержанного storage device; не объявлять неподдержанный режим.
- После успешного durable acknowledgement recovery не может вернуться к состоянию старше последнего подтверждённого commit. Законные последующие overwrite/delete учитываются; это не требование вечно хранить все старые версии. Все ещё referenced snapshots сохраняют собственные данные.
- Если commit мог произойти, но ответ потерян, допустимы только целые состояния, разрешённые transaction protocol. Нельзя обещать rollback по одному факту timeout/flush error. После неопределённого результата остановить новые writes, открыть и reconcile operation ID/generation; blind retry запрещён.
- Атомарность data write определяется контрактом guest device. Многосекторная запись не становится автоматически atomic transaction. Атомарность публикации metadata/root не заменяет data durability.
- Порядок «data → metadata → commit publication → durable ack» обязан быть доказан для выбранного host backend, включая создание/замену файлов и namespace durability. `close`, rename, checksum или один generic sync-вызов сами по себе такой доказательности не дают.
- Host I/O errors, ENOSPC, quota, short write, permission failure и corrupted metadata — явные результаты. Не превращать их в successful zero reads, чистый shutdown или гостевой CPU fault.

### 4.2. CoW и восстановление

- Формат задаёт endian, lengths, versions, feature bits, logical/physical sizes, generation IDs, object/parent identity, checksums и bounds. Проверять overflow до allocation/read; любые parser limits задаются явно.
- Published roots и snapshot parents неизменяемы. Нельзя перезаписывать reachable data/metadata или освобождать их до доказанного отсутствия всех references, readers и save/restore leases.
- Checksum обнаруживает повреждение с установленными ограничениями, но не является authentication, confidentiality или доказательством целостности всего reachable graph. Самый большой generation с валидным header ещё не гарантирует пригодный root.
- Различать interrupted unpublished candidate и corruption уже опубликованного durable состояния. Нельзя молча откатиться на старую «валидную» копию, потеряв подтверждённые данные. При невозможности доказать нормальное recovery открыть diagnostic/read-only или отказать; salvage — отдельная операция.
- Sparse inheritance, explicit zero и discard различимы: удаление child mapping не должно неожиданно раскрывать старые parent bytes. Guest zero semantics и возможность host hole punching определяются отдельно.
- Repair/migration/export сначала создают новый target. Источник не изменяется по умолчанию. Отчёт salvage перечисляет неизвестные/потерянные ranges; подстановка нулей не выдаётся за восстановление исходных bytes.

### 4.3. Снимок VM и идентичность

- Один consistency cut охватывает все vCPU, RAM, device/DMA writers, interrupts, virtual time, pending/completed I/O и persistent disk/UEFI/TPM generations. Disk-only snapshot работающего гостя по умолчанию лишь crash-consistent в заявленных границах.
- Для первой версии сначала остановить новые submissions, завершить принятые external I/O, применить все обязательные completion/DMA effects и остановить writers. Сохранить оставшиеся guest-visible очереди. Не сохранять host socket/file descriptors или kernel handles в надежде воспроизвести их после запуска.
- Если конкретный backend нельзя drain/cancel/reconnect без повторения side effect, checkpoint такого профиля отвергается до публикации. Timeout barrier не разрешает снять «почти остановленную» машину.
- Все компоненты checkpoint сохраняются до атомарной публикации общего manifest с epoch, IDs и hashes. Незавершённая группа не становится selectable snapshot. Это не независимые rename нескольких файлов.
- Restore сначала валидирует полный набор и совместимость, затем создаёт private stopped machine, воссоздаёт ресурсы и только после успеха делает её активной. Первая версия допускает restore/resume только stopped/suspended VM; running/paused session с той же identity даёт busy/conflict. Межпроцессный host session lease по VM identity исключает два активных экземпляра, даже если они открывают разные disk children. Ошибка оставляет прежний persistent root и понятное состояние сессии; никакой частично восстановленной running VM.
- CPU schema включает всю объявленную архитектурную семантику: не только GPR, но необходимые control/segment/descriptor/FP/SIMD/MSR/interruptibility states. JIT cache, native pointers и derived TLB не являются переносимым архитектурным состоянием; восстановить их заново с новыми generations.
- Pause, suspend, guest shutdown, forced stop, disk snapshot, full checkpoint, restore и clone имеют отдельные состояния/ответы. Pause не гарантирует durable data; отключение процесса не доказывает guest shutdown.
- Resume сохраняет идентичность VM. Clone создаёт новую идентичность по явной политике; копирование диска не разрешает молча дублировать TPM identity/secrets, MAC/UUID или UEFI trust state. Fresh TPM может потребовать recovery зашифрованного гостя; доступность данных проверяется отдельно.
- Согласованный откат disk+TPM не гарантирует anti-rollback и не отменяет последствия для внешнего доверия, counters и remote services. Security profile фиксирует допустимые rollback и механизмы их обнаружения, если они заявлены. Recovery credentials не пишутся в Git/логи.

## 5. Стенд и доказательства

Главный продуктовый storage host — Apple Silicon/macOS. Зафиксировать host OS, filesystem, volume/container, media/controller, cache assumptions, свободное место и конкретный durability API. Начальную поддержанную комбинацию выбрать в S9-00; portable unit tests не доказывают её power-loss свойства.

**Единственный x86 — Mac mini с Ubuntu, SSH `192.168.1.14`, текущий login стенда `senya@192.168.1.14`.** Сведения и прежний checkout находятся в [STAND.md](../testing/STAND.md). Использовать Mini для Linux/platform сравнения, переносимости bytes и доступных native сценариев. Linux filesystem/flush на Mini не является oracle гарантий APFS/macOS. Новая x86 машина этому плану не требуется.

Разделять четыре уровня: deterministic fault model; аварийное завершение процесса; авария/перезапуск host; реальная потеря питания выбранного storage stack. Process kill оставляет host caches живыми и не заменяет power cut. Hardware tests выполнять на выделенных одноразовых данных и разрешённом стенде с сохранённым вне него журналом acknowledgement.

В S9-39 сначала подготовить конкретную схему стенда, recovery и перечень затрагиваемых носителей, затем получить разрешение на фактическое отключение/перезапуск. Нельзя выключать рабочий Mac или Mini, форматировать системный диск или отключать чужой накопитель автоматически. Если безопасного power-loss стенда нет, hardware gate остаётся `blocked`/`not_run`; остальные работы продолжаются. Отключение внешнего накопителя проверяет только описанную комбинацию, а не автоматически внутренний SSD Mac.

В рамках подготовки этого плана выполняются только проверки документации и commit; storage experiments, SSH и power-loss tests не запускаются.

## 6. Задачи и минимальные зависимости

Prerequisite в таблице означает принятую необходимую часть среза. Для интеграции дополнительно обязательны реальные компоненты профиля из S9-01. Таблица не требует ждать полного этапа 8 перед offline storage. S9-34 — опциональное расширение; его отсутствие не разрешает claim application-consistent snapshot.

| Задача | Зависимости | Результат |
|---|---|---|
| S9-00 | — | Точный scope и failure model |
| S9-01 | S9-00 | Инвентаризация prerequisites |
| S9-02 | S9-01 | Семантика операций и durable outcomes |
| S9-03 | S9-02 | Внедряемый storage I/O contract |
| S9-04 | S9-03 | Единственный writer и lifetime |
| S9-05 | S9-04 | RAW read/write/reopen |
| S9-06 | S9-05 | Реальный durability path |
| S9-07 | S9-06 | Deterministic fault backend и oracle |
| S9-08 | S9-07 | Собственный bounded CoW format |
| S9-09 | S9-08 | Строгий readonly parser |
| S9-10 | S9-09 | Create/open и root selection |
| S9-11 | S9-10 | CoW write transaction |
| S9-12 | S9-11 | Crash recovery с durable watermark |
| S9-13 | S9-12 | Guest storage/flush integration |
| S9-14 | S9-12 | Неизменяемые snapshots и цепочки |
| S9-15 | S9-14 | Branch/checkout/rollback offline |
| S9-16 | S9-15 | Проверка identities и родителей |
| S9-17 | S9-16 | Удаление и безопасная сборка мусора |
| S9-18 | S9-17 | ENOSPC, zero/discard и лимиты |
| S9-19 | S9-18 | Inspect/check CLI |
| S9-20 | S9-19 | RAW import/export |
| S9-21 | S9-20 | Явный salvage в новый target |
| S9-22 | S9-21 | Версии и безопасная migration |
| S9-23 | S9-02, S9-16 | Контракт полного VM checkpoint |
| S9-24 | S9-23 | CPU/RAM serialization |
| S9-25 | S9-23 | Devices/time/I/O serialization |
| S9-26 | S9-23 | UEFI persistent state |
| S9-27 | S9-23 | vTPM persistent/security state |
| S9-28 | S9-13, S9-24, S9-25, S9-26, S9-27 | Согласованный barrier |
| S9-29 | S9-28, S9-18 | Atomic full-checkpoint publication |
| S9-30 | S9-29 | Проверяемый staged restore |
| S9-31 | S9-30 | Suspend/resume и distinct lifecycle |
| S9-32 | S9-31, S9-22 | Clone и политика identity |
| S9-33 | S9-32 | Host API/VMHost/UI outcomes |
| S9-34 | S9-29, S9-33 | Опциональный guest quiesce/thaw |
| S9-35 | S9-20, S9-30, S9-32, S9-33 | Реальные guest acceptance cases |
| S9-36 | S9-35 | Recovery после аварии VMHost |
| S9-37 | S9-22, S9-36 | Конкурирующие readers/operations |
| S9-38 | S9-37 | Полная fault/corruption matrix |
| S9-39 | S9-38 | Контролируемая потеря питания |
| S9-40 | S9-18, S9-31 | Измерения и performance решения |
| S9-41 | S9-38, S9-40 | Repeated recovery и soak |
| S9-42 | S9-39, S9-41 | Воспроизводимый итоговый набор |
| S9-43 | S9-42 | Независимое итоговое ревью |
| S9-44 | S9-43 | Документация и приёмка этапа |

S9-24…27 принимаются локально по codec/contract tests и готовности реального владельца, без требования уже существующего full checkpoint. Их сквозная проверка находится в S9-29/30/35. Профиль без UEFI/TPM получает отдельный ограниченный gate; он не закрывает полные S9-26/27/28 для Windows. Подготовку процедуры S9-39 начинать заранее; недоступность оборудования не блокирует независимые S9-40/41.

## 7. Пошаговые задания

### S9-00. Зафиксировать область поддержки и failure model

- [ ] Принято после архитектурного ревью.
- **Текущий статус (2026-11-05):** черновики [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md), [VM_STATE.md](../specs/VM_STATE.md) и [STORAGE_RECOVERY.md](../testing/STORAGE_RECOVERY.md) созданы; архитектурное ревью не выполнено. Durability API не подтверждён ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)). Не разрешены лимиты logical size диска, virtual capacity CoW, глубины цепочек и числа snapshots, а также LBA/CoW block size — TBD в [STORAGE_FORMAT §4](../specs/STORAGE_FORMAT.md#4-единицы-размеры-и-лимиты).
- **Где:** будущие `STORAGE_FORMAT.md`, `VM_STATE.md`, `STORAGE_RECOVERY.md`; ссылки на текущую support matrix.
- **Сделать:** выбрать первую host/filesystem/media комбинацию, размеры guest sector/Cow block, limits диска/цепочек, single-writer policy, crash model, acknowledged durability и checksum scope. Размеры/структуры на горячем пути пока гипотеза для замеров S9-08. Определить minimal offline и целевой full-VM profiles.
- **Проверить:** каждый advertised режим имеет source/API основание, сценарий и владельца; network/removable/непроверенные backends не получают случайно те же гарантии.
- **Готово:** понятны границы обычного write, flush, crash recovery, corruption detection и power loss; отсутствие hardware evidence отмечено, а не замаскировано обещанием.

### S9-01. Проверить prerequisites по фактическому коду

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** матрица в [VM_STATE.md §2](../specs/VM_STATE.md#2-инвентаризация-prerequisites-s9-01) составлена по коду; ревью не выполнено; `STAGE_9_REPORT.md` ещё не создан. Следующий runnable срез — S9-03; full-VM prerequisites заблокированы ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)).
- **Где:** отчёт этапа и матрица зависимости в `VM_STATE.md`.
- **Сделать:** перечислить готовность RAW/device backend, guest storage commands, всех CPU/RAM/device/time/I/O states, UEFI variables, vTPM и guest agents. Для каждого обязательного state указать реального owner и его versioned save/load contract либо точный blocker.
- **Проверить:** методы snapshot в testkit и session ABI не приняты за сериализацию всей машины; нет зависимости offline CoW от всей Windows. Нужные задачи этапов 4–8 связаны с roadmap/фактическим кодом, без выдуманных IDs.
- **Готово:** выбран следующий runnable storage slice; full-VM prerequisites явно отделены от него.

### S9-02. Описать операции, состояния и результаты

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** таблицы операций в [VM_STATE.md §3](../specs/VM_STATE.md#3-операции-и-состояния-s9-02); ревью не выполнено.
- **Где:** `VM_STATE.md`, storage contract; control protocol меняется только при интеграции S9-33.
- **Сделать:** таблица для pause/resume, suspend, guest shutdown, forced stop, disk-only snapshot, full checkpoint, restore, clone, export, repair. Для каждой: prerequisites, consistency, durability, allowed transitions, cancellation point, operation ID и recoverable/uncertain failure.
- **Проверить:** потеря ответа после commit не приводит к повторному destructive действию; pause не называется suspend; shutdown timeout не выдаётся за успешный clean shutdown.
- **Готово:** состояния после отказа однозначны и проверяемы; pending/committed/acknowledged различимы.

### S9-03. Ввести узкий storage I/O interface

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** реализовано, ревью пройдено, коммит `a59338a`. Contract suite исполняется на memory и host file backend; детали и ограничения — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-03-узкий-storage-io-interface--реализовано-ревью-пройдено).
- **Где:** `od-storage`, host implementation в `od-host`, локальный test backend.
- **Сделать:** checked offset reads/writes, size/truncate/allocate при необходимости, file/namespace durability и locking capabilities; типизированные short/error outcomes. Interface описывает необходимые гарантии, а не копирует весь filesystem API.
- **Проверить:** overflow, short read/write, interrupted call, no progress, несогласованные capabilities; upper library не импортирует host и не скрывает ошибку в zero-filled block.
- **Готово:** один test backend и один настоящий поддержанный host backend исполняют одинаковый contract; raw host handles не становятся persistent fields.

### S9-04. Обеспечить единственного writer

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** частично, коммит `ba77773`: межпроцессный exclusive writer lock (`flock`) с проверкой identity. Не сделаны read leases, интеграция с VMHost, запрет изменения parent, busy при export/repair; lock advisory. Задача не принята — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-04-единственный-writer--частично).
- **Где:** `od-storage` ownership, `od-host` file locking, VMHost lifecycle.
- **Сделать:** exclusive writer ownership для backing и mutating metadata; стабильная file/object identity, read leases и освобождение при shutdown/crash. Path string и PID-файл сами по себе недостаточны; same-file aliases учитывать по поддержанной платформе. Один timeout или возраст lock-файла не разрешает отобрать ownership у живого writer.
- **Проверить:** два процесса, повторное открытие через alias, writer после crash, stale handle и запрещённое изменение parent, занятый диск при export/repair.
- **Готово:** две VMHost не могут одновременно писать один backing; enforcement работает между процессами, не только в одной Rust ownership graph.

### S9-05. Реализовать RAW backend

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** реализовано, ревью пройдено, коммит `4fcd5c4`. Power-loss durability не заявляется; детали — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-05-raw-backend--реализовано-ревью-пройдено).
- **Где:** `od-storage`, local tests, малые собственные fixtures.
- **Сделать:** create/open/read/write/reopen с явным logical size, bounds и разрешённым alignment. Политика create existing file — безопасный отказ, а не молчаливое truncate. Различить out-of-range и чтение неполного файла.
- **Проверить:** начало/конец, unaligned допустимые обращения, partial errors, EOF, повторное открытие и independent byte comparison.
- **Готово:** реальный файл содержит ожидаемые bytes; обычный write completion ещё не объявляется power-loss durable.

### S9-06. Провести durability от storage до host

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** частично, коммит `69e0b02`: только durability данных файла на macOS через `F_FULLFSYNC` и sticky reconcile-required. Namespace durability не реализована ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)), power-loss не проверен, трактовка ENOTSUP/EINVAL — защитное предположение. Задача не принята — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-06-durability-от-storage-до-host--частично).
- **Где:** `od-storage`, `od-host`, методика и отчёт.
- **Сделать:** реализовать выбранный в S9-00 порядок data/metadata/namespace persistence с проверенными API. Зафиксировать volume/medium assumptions и что именно обещает successful flush. Для unsupported backend отказать в заявленной durable операции.
- **Проверить:** flush error до/после потенциального commit, short write, delayed completion, file creation/rename publication; лог операций подтверждает порядок. Process-kill/reopen — отдельный уровень, не power-loss proof.
- **Готово:** durable ack выдаётся только после всех требуемых действий; ambiguous failure переводит writer в reconcile-required state.

### S9-07. Сделать fault backend и независимый oracle

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** реализовано, ревью пройдено (нет блокеров/мажоров), коммит `a7874b8`. Namespace и реальный power-loss в модель не входят — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-07-fault-backend-и-независимый-oracle--реализовано-ревью-пройдено-нет-блокеров-и-мажоров).
- **Где:** unit tests `od-storage`, общая orchestration `tests/system/` и при необходимости `od-testkit` без production dependency.
- **Сделать:** модель volatile/durable bytes и namespace, loss/reorder/torn/short writes в пределах failure model, errors на sync/rename/allocate. Вне faulted target вести sequence и durable watermark; ожидаемое состояние считать простой отдельной логической моделью.
- **Проверить:** специально сломанный flush order/ack должен обнаруживаться; сохранять seed и operation trace; не сравнивать reader с writer, использующими одну ошибку parser.
- **Готово:** первый RAW write→flush→crash→reopen case доказывает ровно заявленную гарантию, а unacknowledged outcomes описаны явно.

### S9-08. Выбрать минимальный собственный CoW format

- [ ] Принято после архитектурного ревью и нужных прототипных замеров.
- **Текущий статус (2026-11-05):** прототип реализован и проверен, ревью протокола пройдено, коммит `000c1da`; формат НЕ принят (гипотезы: размер блока, карта; замеры S9-40 не сделаны), нормативных bytes и лимитов нет — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-08-минимальный-собственный-cow-format--прототип-реализован-и-проверен-формат-не-принят).
- **Где:** `STORAGE_FORMAT.md`, `od-storage`, ADR при выборе среди вариантов.
- **Сделать:** versioned superblock/root descriptors, block map, generation/transaction IDs, checksums и bounded snapshot metadata. Первый кандидат: один файл и writer, фиксированная virtual capacity/block size, append-only immutable extents, полная bounded map поколения и два root slots. Протокол-кандидат: data/map → durability barrier → неактивный root → durability barrier → ack. До writer доказать все cuts; не предполагать atomic sector write и не добавлять journal/B-tree/refcounts без необходимости.
- **Проверить:** на бумаге и минимальном fault prototype разобрать каждый durability cut. Для размера blocks/map и hot path сравнить минимальные варианты на одинаковой нагрузке; выбрать простейший удовлетворяющий измеренным требованиям.
- **Готово:** normative bytes, limits, root selection, required fences, unreachable garbage и несовместимые features определены. Формат не зависит от Rust ABI.

### S9-09. Написать строгий readonly parser

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** реализовано, ревью пройдено (нет блокеров/мажоров, minor в сообщении коммита `17c9ce6`); fuzz 80000 разборов без паник. Формат НЕ принят (S9-40) — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-09-строгий-read-only-parser--реализовано-ревью-пройдено-нет-блокеров-и-мажоров-minor-в-сообщении-коммита).
- **Где:** `od-storage`, малые manually specified fixtures.
- **Сделать:** декодирование header/root/map с version/capability negotiation, проверкой lengths/offset arithmetic/size limits, checksum и references. Unknown mandatory feature запрещает writable open; readonly допустим только при доказанно понятной семантике.
- **Проверить:** truncation, overflow, overlap, duplicate/conflicting entries, wrong endian/version/identity, cycle/depth limits. Expected bytes независимы от encoder.
- **Готово:** повреждённый input не вызывает host panic, огромную allocation или неограниченный обход; ошибка имеет точное место и тип.

### S9-10. Реализовать создание и выбор корня

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** реализовано, ревью пройдено после одного major (исправлен), коммиты `61a99e0`, `88b9f42`. Открытый предел: [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable). Формат НЕ принят (S9-40) — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-10-createopen-и-выбор-корня--реализовано-ревью-пройдено-после-одного-major-исправлен-есть-открытый-предел-issue-057).
- **Где:** `od-storage`.
- **Сделать:** create пустого диска и open только полностью валидируемого состояния. Root validation включает reachable metadata/data integrity в заявленном checksum scope; generation и publication status проверяются по S9-08.
- **Проверить:** power cut model на каждом шаге create/publication, damaged root copies, same generation conflict, generation overflow, валидный header с невалидной map, отсутствие пригодного root. Для маленького correctness profile проверить весь reachable graph; lazy validation вводить отдельно по замерам.
- **Готово:** reader различает unfinished candidate, coherent committed state и corruption/ambiguity. Torn inactive root допустим лишь в модели, где предыдущий committed root не повреждён. Произвольная порча последнего durable root может быть неотличима от torn candidate: если latest state установить нельзя, normal writable open отказывает, а older-root salvage явно сообщает риск потери. Checksum-valid newest root с повреждёнными dependencies не разрешает тихий fallback.

### S9-11. Реализовать CoW write transaction

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** реализовано, ревью пройдено (один major исправлен), коммит `7d23bf9`. Не покрыто: real power loss, reclaim/GC, snapshots; формат НЕ принят (S9-40), открыт [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-11-cow-write-transaction--реализовано-ревью-пройдено-один-major-исправлен).
- **Где:** `od-storage` writer и transaction tests.
- **Сделать:** подготовить новые extents/map, обеспечить нужную durability и опубликовать root по protocol; published objects неизменяемы. Ограничить размер transaction/dirty state; cancellation до/после publication имеет разные результаты.
- **Проверить:** overwrite, partial logical block, несколько extents, flush boundary, commit error и reader view. Старая root generation остаётся пригодной до разрешённого reclaim.
- **Готово:** read после успешного commit видит целую новую mapping; durable ack не предшествует persistent dependencies; uncertain result нельзя retry без reconcile.

### S9-12. Проверить crash recovery по durable watermark

- [ ] Принято после ревью.
- **Текущий статус (2026-11-05):** реализовано (recovery tests), ревью пройдено, коммит `7d23bf9`. Не покрыто: real power loss, reclaim/GC, snapshots; формат НЕ принят (S9-40), открыт [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) — [STAGE_9_REPORT.md](../reports/STAGE_9_REPORT.md#s9-12-crash-recovery-по-durable-watermark--реализовано-recovery-tests-ревью-пройдено).
- **Где:** storage recovery, fault harness, report/data.
- **Сделать:** восстановление после каждого допустимого cut S9-11, cleanup только доказанного garbage; side-effect-free inspect перед возможной mutation. Сравнивать с последним durable acknowledgement и разрешёнными более поздними outcomes.
- **Проверить:** repeated reopen/recovery, crash во время recovery, delayed writes, corrupt committed extent и unack-but-committed root. Старый valid root с потерянным acknowledged write обязан провалить oracle.
- **Готово:** recovery идемпотентен в объявленной семантике; unexplained mismatch не превращается в tolerated data loss.

### S9-13. Подключить guest storage и flush

- [ ] Принято после ревью; нужен реально работающий device prerequisite из S9-01.
- **Где:** storage device/DMA/queues в `od-machine`, backing в `od-storage`, host adapter в `od-host`.
- **Сделать:** провести поддержанные read/write/flush/barrier/FUA от guest command до backend и completion. Если устройство из этапа 5/7 ещё отсутствует, реализовать его отдельный prerequisite по roadmap, не назвать host API integration готовым NVMe.
- **Проверить:** порядок нескольких queues, completion до/после flush, guest error при host failure, DMA/interrupt effects ровно один раз. Advertised volatile-cache/atomicity features соответствуют реализации.
- **Готово:** guest-visible completion имеет документированную durability; fake device fixture отдельно помечен и не закрывает сквозной gate.

### S9-14. Добавить неизменяемые snapshots и цепочки

- [ ] Принято после ревью.
- **Где:** `od-storage`, `STORAGE_FORMAT.md`.
- **Сделать:** snapshot ID/root/parent generation, bounded chain, read resolution, immutable parent и создание writable child. Зафиксировать content и metadata identity родителя; не доверять одному относительному имени файла.
- **Проверить:** child overwrite не меняет parent; чтение holes/zeros; нескольких snapshot roots; missing parent, цикл и превышение depth. Snapshot metadata publication переживает fault injection.
- **Готово:** snapshot воспроизводит зафиксированные logical bytes после новых writes и reopen; наследование не раскрывает будущие parent mutations.

### S9-15. Branch, checkout и disk rollback offline

- [ ] Принято после ревью.
- **Где:** `od-storage`, image operations contract.
- **Сделать:** из выбранного immutable snapshot создавать новый child, переключать active root атомарно; изменяемый guest диск не заменять под работающей VM. Disk rollback не изменяет RAM/TPM/UEFI автоматически.
- **Проверить:** old/new whole outcome при crash, сохранность остальных branches, wrong active generation и live-writer refusal.
- **Готово:** можно вернуться к выбранным logical bytes без разрушения источника; операция не маркируется full-VM restore.

### S9-16. Проверить parent identities и leases

- [ ] Принято после ревью.
- **Где:** storage chain resolver и host file access boundary.
- **Сделать:** validate всю выбранную цепочку до writable open; hold нужные parent/read leases; порядок locks исключает deadlock. File alias/replacement и path traversal не дают подменить parent между check и use.
- **Проверить:** неправильный parent hash/ID/generation, symlink/alias в заявленном scope, TOCTOU replacement, concurrent child open и incomplete copy bundle.
- **Готово:** parent нельзя удалить/мутировать, пока от него зависит открытый child или published snapshot; неподтверждённый provenance цепочки даёт явный отказ.

### S9-17. Удаление снимков и reclamation

- [ ] Принято после ревью.
- **Где:** `od-storage`; ownership protocol не переносить в UI.
- **Сделать:** отделить удаление имени snapshot от освобождения data; rootset включает named snapshots, writable head, обе recoverable root generations, saved-VM manifests, clone parents и reader/export leases. Сначала не переиспользовать extents; первая reclamation — offline exclusive mark и copy-compaction в новый файл с полной проверкой и durable publication. In-place reuse или concurrent GC требуют отдельного доказанного среза.
- **Проверить:** shared parent, active child, pinned export, crash до/после tombstone/reclaim; повторная recovery не освобождает живые blocks. Old readers не получают reused extent. После copy-compaction logical identity и все parent/manifest references проверены заново; изменение physical offsets не разрешает подменять immutable parent без согласованного перевода consumers.
- **Готово:** reclaim только доказанно unreachable объектов; исходный файл copy-compaction сохранён до подтверждённого результата и освобождения всех удерживающих его ссылок. External clone/VM reference либо переведён проверяемой операцией, либо продолжает удерживать source; неизвестный потребитель запрещает destructive reclaim. Snapshot deletion в текущем root не освобождает bytes, на которые ссылается предыдущий recoverable root. Потеря свободного места допустима как диагностируемый garbage, потеря reachable bytes — нет.

### S9-18. Capacity, explicit zero и discard

- [ ] Принято после ревью.
- **Где:** storage allocation, guest storage feature contract.
- **Сделать:** ENOSPC/quota/reserved space policy, bounded metadata growth и операции zero/discard в заявленном scope. Discard в child должен соблюдать guest contract, не неявно возвращать parent data; host hole punching использовать только как проверенную оптимизацию.
- **Проверить:** disk full на data/map/root/reclaim, sparse extent, partial block zero, parent masking, reopen и flush. Нельзя обещать rollback уже опубликованной transaction после последующего ENOSPC.
- **Готово:** ресурсный отказ не портит предыдущий durable state; неподдержанный discard не рекламируется, zero semantics проверены независимо.

### S9-19. Создать inspect/check CLI

- [ ] Принято после ревью.
- **Где:** `tools/image-tool/src/` поверх `od-storage`; layout/manifests обновлять при реальной сборке tool.
- **Сделать:** readonly info/check с version, logical size, root/chain IDs, integrity outcomes и машинно различимыми exit statuses. Mutating recovery — отдельная явная операция; default inspect источник не меняет.
- **Проверить:** valid/corrupt/unsupported/busy input, bounded output, отсутствие секретов и гостевых payload в diagnostics.
- **Готово:** CLI использует единственный library parser и позволяет объяснить причину отказа open; checker не делает silent repair.

### S9-20. RAW import/export с проверкой результата

- [ ] Принято после ревью.
- **Где:** `od-storage`, `tools/image-tool`, test fixtures.
- **Сделать:** import в новый собственный image; export выбранного immutable root/chain в простой RAW точного logical size. Зафиксировать source generation/lease на весь export, не читать меняющийся active disk; publish target только после завершения проверки/durability.
- **Проверить:** block-by-block независимое сравнение/hash, holes/zeros/последний неполный block, cancellation, ENOSPC, existing target и crash. Не публиковать частичный файл как completed export.
- **Готово:** RAW читается независимым byte reader и содержит ровно данные выбранного snapshot; переход через наш parser дважды не единственное доказательство.

### S9-21. Сделать отдельный salvage/recovery-to-copy

- [ ] Принято после ревью.
- **Где:** `od-storage`, image-tool, методика восстановления.
- **Сделать:** определить, какие данные можно доказанно извлечь при повреждении metadata; копировать в новый target и выпускать extent/report с valid/unknown/lost/conflicting ranges и основаниями. Сложный reconstruction вне первой версии можно явно отказать.
- **Проверить:** повреждённая map/root, valid stale blocks, conflicting generations и неизвестный parent; не включать случайно найденные старые bytes как актуальные без provenance.
- **Готово:** источник сохранён, отчёт честно описывает recoverability. Если невозможно восстановить требуемую metadata по выбранному формату, это открытый критерий, а не success с нулевыми данными.

### S9-22. Версии формата и migration

- [ ] Принято после ревью.
- **Где:** `od-storage` codecs, `STORAGE_FORMAT.md`, image-tool.
- **Сделать:** compatibility table read/write/upgrade/downgrade; unknown mandatory features отклонять. Migration сначала в новый target, с проверкой logical data, snapshots/parent links и identities; activation отделить от copy. Не обещать произвольный downgrade.
- **Проверить:** tiny frozen fixtures поддержанных старых versions, interrupted migration, source untouched, recovery после publish-before-response, insufficient space и unsupported version.
- **Готово:** безопасное обновление воспроизводимо и не требует перезаписи единственной рабочей копии; legacy input не маскируется новой версией header.

### S9-23. Определить versioned full-VM container и manifest

- [ ] Принято после архитектурного ревью.
- **Где:** `VM_STATE.md`, container codec в `od-storage`, participant contract `od-machine`.
- **Сделать:** checkpoint ID/epoch, VM identity/config profile, CPU count/features, section schemas/lengths/checksums, RAM ranges, device versions, time/I/O state, disk roots и UEFI/vTPM refs. Manifest связывает весь набор; UI catalog остаётся восстанавливаемым.
- **Проверить:** missing/duplicate/unsupported sections, total size limits, inconsistent epochs/IDs и required participant. Reader не пропускает unknown mandatory state.
- **Готово:** список участников полный для выбранного профиля; формат не содержит host addresses/handles; local manifest tests не требуют ещё не созданного running checkpoint.

### S9-24. CPU и RAM save/load contract

- [ ] Принято после ревью; нужны реальные архитектурные state owners.
- **Где:** `od-isa`, `od-machine`; storage пишет sections по контракту, не определяет CPU-семантику.
- **Сделать:** explicit versioned CPU fields всего профиля и RAM ranges/bytes; validation logical addresses/sizes/reserved fields. Guest page size не равен host page size. Для первой версии stopped full-RAM copy проще live dirty tracking.
- **Проверить:** semantic roundtrip и independently expected fields, несколько vCPU при объявленном SMP, sparse RAM, 4 KiB guest внутри 16 KiB host page; unsupported CPU feature/state нельзя терять.
- **Готово локально:** настоящий CPU/RAM owner сохраняет и загружает полный объявленный state; latent data не заменены zero defaults. Execution continuation проверяется позже S9-30/35.

### S9-25. Устройства, время и I/O state

- [ ] Принято после ревью; нужны реальные модели устройств.
- **Где:** `od-machine`, narrow host resource contracts.
- **Сделать:** inventory каждого device: registers, queue heads/tails, IRQ pending/in-service, DMA, timer deadlines и virtual-clock policy. Определить drained external operations, сериализуемые guest queues и reconnect/reset semantics сети/ввода/прочих внешних ресурсов.
- **Проверить:** busy device, completion перед/после barrier, pending interrupt, paused timer, absent backend; нельзя replay external write/send дважды. Недренируемый passthrough/device блокирует snapshot профиля.
- **Готово локально:** каждый participant имеет save/validate/load и resume order; timeout/error не считается пустой очередью.

### S9-26. Включить UEFI variables как отдельного участника

- [ ] Принято после ревью; нужна реализованная UEFI variable semantics этапа 7.
- **Где:** firmware state contract в `firmware/uefi/src/`, host persistence/container в `od-storage`, coordination `od-machine`.
- **Сделать:** versioned variable store, attributes, authenticated-update metadata и trust policy согласно поддержанному профилю; capture immutable generation во время общего cut. Firmware остаётся отдельной guest-target сборкой.
- **Проверить:** concurrent variable operation, partial save, wrong identity/version, rollback authenticated/trust state и зависимости от внешних anchors. Не переносить семантику variables в generic disk parser.
- **Готово локально:** реальный variable-store codec и restore policy проверены; разрешённые rollback и запреты перечислены. Отсутствующая UEFI не заменяется пустым blob для Windows gate.

### S9-27. Включить vTPM и определить security/rollback policy

- [ ] Принято после архитектурного и независимого ревью; нужен собственный работающий vTPM.
- **Где:** TPM state owner `od-machine`, persistent serialization `od-storage`, host key protection `od-host`.
- **Сделать:** полный перечень persistent/volatile TPM state по profile, sessions/counters/keys и external anchors; согласованный capture с диском/UEFI. Определить confidentiality/access control для TPM secrets и RAM snapshots, ключевые зависимости и поведение при отсутствии host secret.
- **Проверить:** mixed disk/TPM epochs, corrupted/protected state, wrong VM identity, unavailable key, counter rollback и restore на другой host в заявленных границах. Подготовить recovery encrypted guest; secrets не логировать.
- **Готово локально:** policy честно различает coherent rollback, обнаружение rollback и доверие внешних систем; fresh TPM и восстановленный TPM — разные операции. Никакой anti-rollback гарантии из одной checksum.

### S9-28. Реализовать общий barrier vCPU/I/O

- [ ] Принято после ревью для заявленного профиля.
- **Где:** `od-machine`, участники storage/devices/CPU/JIT, host completion boundary.
- **Сделать:** фиксированный порядок stop submissions, safe points всех vCPU, drain accepted external I/O, apply completion/DMA, остановка остальных writers/time и capture epoch. Проверить lock order; JIT материализует CPU state, не оставляет hidden live registers.
- **Проверить:** one/multi-vCPU только в реально поддержанном scope, racing completion, event deadline, STOP/cancel и timeout каждого participant. Abort до publication возобновляет согласованную прежнюю сессию либо оставляет явное stopped/failed state.
- **Готово:** после barrier ни один участник не меняет captured generation; частичное согласие участников никогда не обозначается coherent checkpoint.

### S9-29. Сохранить всю VM и атомарно опубликовать manifest

- [ ] Принято после ревью.
- **Где:** coordination `od-machine`, container/group publication `od-storage`, host durability adapter.
- **Сделать:** под одним cut сохранить CPU/RAM/devices/time/I/O и immutable refs всех disks/UEFI/TPM; durable каждый обязательный artifact, затем общий manifest и acknowledgement. Не требовать atomic rename нескольких файлов как несуществующую общую transaction.
- **Проверить:** fault на каждом participant и publication step, mixed epochs, несколько disks, disk-full при RAM save, crash между commit и response; incomplete groups не selectable и не освобождают pinned data.
- **Готово:** опубликованный checkpoint целиком валиден и durable по профилю; orphan cleanup отдельно безопасен; resume running VM после save не изменяет frozen checkpoint.

### S9-30. Реализовать staged restore

- [ ] Принято после ревью.
- **Где:** `od-machine`, component validators, `od-storage`, host resource lifecycle.
- **Сделать:** сначала verify всю dependency graph и compatibility; отклонить restore running/paused VM в первой версии. До создания target зарезервировать identity узким межпроцессным host session lease; stopped прежний VMHost должен освободить/передать право активации, старый owner generation утрачивает это право. Затем создать private stopped machine и восстановить участников. Для writable продолжения создать новые children/persistent generations от immutable checkpoint. Rebuild TLB/JIT/cache/handles; activate только после общего успеха и проверки единоличного identity lease.
- **Проверить:** incompatible CPU/device/schema, missing parent/key, allocation/host-resource error, wrong epoch, stale executable handle и failure на середине restore, включая прерванную activation после publication (известное состояние после отказа: `failed_after_publication`/`interrupted_activation`, VM `stopped` с новыми generations, см. [VM_STATE §3.2](../specs/VM_STATE.md#32-operation-id)); два разных restore/resume requests одной VM, живой прежний VMHost и crash при передаче lease. Разные disk children не разрешают одновременно запустить одинаковые UUID/MAC/vTPM identities. Не запускать CPU частично восстановленной VM.
- **Готово:** либо активировано полное нужное состояние с одним owner, либо target не запущен и исходные artifacts не повреждены; состояния старой/новой sessions после отказа известны. Host lease действует в поддержанном локальном scope, без нового daemon/distributed locking. Derived host state не переживает restore как сохранённые pointers.

### S9-31. Suspend/resume и различимые lifecycle operations

- [ ] Принято после ревью.
- **Где:** `od-machine`, VMHost lifecycle, `VM_STATE.md`.
- **Сделать:** suspend = успешный durable full checkpoint и затем освобождение running resources; resume = validated restore с прежней VM identity. Pause только останавливает execution по контракту, guest shutdown ожидает guest path, forced stop имеет отдельный outcome. Определить reuse/consumption saved state и безопасный повтор после потерянного ответа.
- **Проверить:** suspend failure до/после publication, process exit, repeated resume request, resume при уже running identity, STOP race, guest shutdown timeout и no-op повтор. После crash владение проверяется через S9-30 lease, а не только command ID. Не уничтожать единственный пригодный checkpoint до успешного durable handoff.
- **Готово:** операция после crash имеет recoverable stable ID/state; UI может сообщить правду без угадывания по наличию файла.

### S9-32. Clone и новая идентичность

- [ ] Принято после архитектурного ревью.
- **Где:** `od-machine` clone orchestration, `od-storage`, firmware/TPM policy; UI позже вызывает этот контракт.
- **Сделать:** явная policy для VM UUID, MAC, disk IDs, UEFI variables/trust и vTPM. По умолчанию clone не дублирует TPM identity/secrets; сохранить provenance source и новые независимые writable generations. Ограниченный clone можно делать из stopped disk/template; перенос live CPU/RAM под новую identity отдельно обосновать или отклонить.
- **Проверить:** независимые writes source/clone, shared immutable parent retention, interrupted clone, identity collision, encrypted guest recovery с fresh TPM. Если нужных recovery credentials нет, не обещать boot/unlock.
- **Готово:** resume и clone нельзя перепутать; исходная VM не меняется; заявленный encrypted-guest сценарий имеет реальные результаты без утечки ключей.

### S9-33. Подключить API, VMHost и минимальное управление

- [ ] Принято после ревью.
- **Где:** `od-host`, `include/open_desktop/`, `apps/vmhost/Sources/`, `protocols/control/`, минимальные UI actions.
- **Сделать:** команды и события отдельных S9-02 операций, IDs, progress, committed/uncertain outcome, compatibility errors и recovery query. Использовать существующий C ABI/versioning; нормативные bytes/headers в их владельцах.
- **Проверить:** потеря IPC, повтор команды, reconnect после commit, crash VMHost, busy identity lease и stale UI catalog; UI не пишет image напрямую и не запускает автоматический destructive retry.
- **Готово:** headless/API и UI показывают одни фактические outcomes. Большой новый VM wizard и остальные функции этапа 11 не входят в задачу.

### S9-34. Опциональный guest quiesce/thaw

- [ ] Принято после ревью либо явно исключено из первого профиля.
- **Где:** guest agents, `protocols/guest/`, orchestration `od-machine`.
- **Сделать:** versioned handshake с capability, request ID, timeout/lease, freeze/quiesce acknowledgement и гарантированным thaw/recovery path. Поддержанный guest filesystem/application provider указывать точно; один filesystem flush не доказывает согласованность базы данных.
- **Проверить:** agent absent, timeout, partial freeze, host crash, checkpoint failure и repeated thaw; guest не остаётся навсегда замороженным при потере host.
- **Готово:** application-consistent claim разрешён лишь для реально проверенного provider/workload. Без него S9-29 остаётся coherent VM/crash-consistent storage в установленной семантике.

### S9-35. Выполнить реальные Linux/Windows сценарии

- [ ] Принято после ревью для каждого обязательного guest profile.
- **Где:** `tests/system/`, `tests/compat/`, малые собственные guest fixtures, report/data.
- **Сделать:** на реально работающих OS из принятых этапов 5/8 проверить numbered file writes+flush, filesystem reopen, disk snapshot, full checkpoint/restore, suspend/resume, UEFI/vTPM continuity и encrypted guest recovery. Базовые offline задачи не ждут этого шага.
- **Проверить:** hashes и sequence counters, гостевые completions/interrupts после resume, timer policy, несколько disks, restart приложения. Expected durable acknowledgements сохранять вне faulted VM; guest success marker один не достаточен.
- **Готово:** реальные guest results отделены от fixtures; отсутствующая Windows/TPM означает blocked соответствующего gate, а не completion по Linux smoke.

### S9-36. Восстановление после аварии VMHost

- [ ] Принято после ревью.
- **Где:** VMHost, storage recovery, system harness.
- **Сделать:** завершать отдельный disposable VMHost на разных стадиях write/snapshot/suspend/restore/clone и заново reconstruct state из persistent bundle. Восстановить UI catalog как индекс; определить uncertain operations по IDs/manifest.
- **Проверить:** смерть до/после commit и до response, отсутствие двойного external side effect, release/reacquire locks, partial cleanup и повторное восстановление.
- **Готово:** приложение не молча перезапускает mutating operation; process-kill evidence явно не называется power-loss испытанием.

### S9-37. Конкурирующие readers и операции

- [ ] Принято после ревью.
- **Где:** storage/VM lifecycle tests, multiprocess harness.
- **Сделать:** разрешённые сочетания run/read snapshot/export/delete/migrate/restore/clone; конфликтующие mutations возвращают busy/conflict либо сериализуются по contract. Определить lock ordering для нескольких disks и persistent participants.
- **Проверить:** export+GC, child writer+parent deletion, same-file alias, two VMHost, stale generation, timeout/cancel и lock holder crash. Read lease удерживает именно выбранную generation.
- **Готово:** нет deadlock, TOCTOU mutation или освобождения pinned state; запрещённые комбинации не зависят от скорости race.

### S9-38. Полная матрица fault injection и повреждений

- [ ] Принято после ревью.
- **Где:** `tests/system/`, local parser/writer tests, методика/малые regression fixtures.
- **Сделать:** перебрать реальные persistence cut points всех mutating операций; добавить short/torn/lost/reordered writes, corrupted lengths/root/map/payload, ENOSPC/I/O errors и interruption recovery. Fuzz parser bounded inputs; seeds/repro сохранять.
- **Проверить:** independent oracle сохраняет последний durable watermark и referenced snapshots; valid-but-stale root не даёт PASS. Corruption вне обещанной repair модели обнаруживается явно, а не «восстанавливается» silent rollback.
- **Готово:** все найденные mismatches объяснены и исправлены либо отмечены блокирующим дефектом. Ноль crashes parser не заменяет доказательства данных.

### S9-39. Контролируемые power-loss испытания

- [ ] Процедура принята; реальные испытания выполнены и приняты отдельно.
- **Где:** `STORAGE_RECOVERY.md`, disposable стенд и report/data.
- **Сделать:** заранее подготовить схему питания/storage stack, одноразовый volume, recovery и внешний журнал durable acknowledgements. После конкретного разрешения пользователя прерывать питание на выбранных стадиях, восстанавливать и проверять данные/manifest/TPM scope. Не использовать рабочие данные или системный диск как расходный материал.
- **Проверить:** реальное отключение целевого persistence domain, повторяемость, model/firmware/controller/filesystem, число прогонов и полный raw result. Linux Mini по SSH можно использовать только в его отдельно разрешённом scope; его результат не заменяет macOS backend.
- **Готово:** есть реальные результаты для заявленной поддержанной комбинации. Если оборудования/разрешения нет — `blocked`/`not_run`; процесс kill и симуляция остаются отдельными уровнями evidence.

### S9-40. Измерить стоимость корректных операций

- [ ] Принято после ревью.
- **Где:** `tools/bench/`, отчёт и small data.
- **Сделать:** RAW baseline против CoW на одинаковых размерах/flush guarantees; sequential/random read/write, durable latency distribution, write amplification, chain depth, fragmentation, export/reclaim, checkpoint pause/duration, restore и память. Повторные прогоны, cold/warm, realistic dirty data.
- **Проверить:** одинаковая correctness/durability, hardware/FS/build fingerprint, variance и retained disk space. Нельзя получить ускорение отключением flush, checksums или barrier; не принимать сложную структуру только по asymptotic claim.
- **Готово:** performance decisions основаны на данных. Если цель не достигнута, записать ограничение и конкретный следующий эксперимент, не снижая гарантий.

### S9-41. Repeated recovery и длительные циклы

- [ ] Принято после ревью.
- **Где:** system/soak harness и report/data.
- **Сделать:** повторять write/flush/snapshot/branch/delete/export/suspend/restore/clone с периодическими failures, chain growth и memory/disk limits. После каждого recovery заново проверить latest durable state и оставшиеся snapshots.
- **Проверить:** утечки пространства/handles, generation overflow policy, stale events после restore, repeated migration, восстановление после сбоя самого recovery. Длительность и число циклов задать до прогона.
- **Готово:** нет необъяснённых потерь/повреждений в измеренном scope; ограниченный soak не объявляется абсолютной гарантией отсутствия ошибок.

### S9-42. Собрать воспроизводимый итоговый набор

- [ ] Принято после ревью.
- **Где:** методика, существующие tools, отчёт и support matrix.
- **Сделать:** freeze commit/config/artifact hashes и выполнить offline storage, real guest, lifecycle, corruption/crash, repeated recovery и power-loss matrix. Для optional S9-34 либо добавить выполненные provider tests, либо явно исключить соответствующий claim.
- **Проверить:** каждый case имеет `passed`/`failed`/`not_run`/`blocked`, environment и evidence reference; большие images/logs в ignored `work/stage9/<run-id>/`, в Git только малые собственные fixtures/отчёты без секретов.
- **Готово:** любой mandatory blocked gate сохраняется видимым; local success не заполняет hardware/Windows клетки автоматически.

### S9-43. Независимое итоговое ревью

- [ ] Все обязательные замечания закрыты.
- **Кто:** `gpt-6.1-sol` / `high`; архитектурные разногласия — `gpt-6-astra` / `high`.
- **Проверить:** формат/limits, persisted ordering, ack uncertainty, root selection и salvage, ownership/leases/reclaim, whole-VM epoch/barrier, restore activation, UEFI/vTPM trust/clone, API outcomes и честность evidence. Ревьюер анализирует diff и contracts, а не только зелёный summary тестов.
- **Готово:** подтверждённые проблемы исправлены с повторной проверкой. Если independent review недоступен, записать это прямо; не ставить ему passed.

### S9-44. Документировать приёмку и оставшиеся ограничения

- [ ] Этап принят по gates раздела 8.
- **Где:** этот план, `STAGE_9_REPORT.md`, `ROADMAP.md`, затронутые architecture/layout/specs/testing/support matrix и `KNOWN_ISSUES.md`.
- **Сделать:** записать реальные гарантии и форматы, поддержанные host/guest combinations, команды check/export/recovery, migration/rollback/clone policy, доказательства и результаты ревью. Открытые дефекты получают scope, последствия, воспроизведение/основание и следующий шаг.
- **Проверить:** документация не называет disk snapshot backup всех данных VM или filesystem freeze application consistency без evidence; все локальные ссылки и структуры корректны.
- **Готово:** следующий исполнитель может восстановить disposable VM по инструкции и понимает, какие действия запрещены/неподдержаны и какие данные обязательны для восстановления.

## 8. Итоговые gates

| Gate | Обязательное доказательство | Недостаточная замена |
|---|---|---|
| G1 — independent storage | S9-03…12: RAW/CoW, durability protocol и recovery без потери durable watermark | Compile-only; открывается любая старая валидная root |
| G2 — snapshots/tools | S9-14…22: immutable chains, safe reclaim, export, явный salvage, version migration | Удаление файла snapshot без проверки references; нулевой RAW вместо утраченных bytes |
| G3 — coherent full VM | S9-13, 23…31: реальный barrier, полный state, atomic group publication, staged restore | GPR+RAM fixture; независимые snapshots диска и TPM |
| G4 — identity/integration | S9-32…37: clone policy, lifecycle, реальные OS, recovery и concurrent operations | Clone с молча скопированными ключами; один marker без continuation evidence |
| G5 — failure qualification | S9-38/39/41: deterministic faults, crashes, repeat recovery, реальные power-loss tests | Kill процесса вместо power cut; непроверенная host FS комбинация |
| G6 — reproducibility | S9-40…44: одинаковые durability guarantees в замерах, полная matrix и ревью | Ускорение ценой потери flush; только финальный ответ в чате |

Полный этап закрывается после всех mandatory gates для согласованной начальной support matrix. Пример честного частичного результата: «offline CoW и export приняты; full-VM blocked до vTPM; macOS power-loss not_run». Не расширять claim по факту наличия структуры каталогов. Optional guest coordination исключается только вместе с соответствующим application-consistency claim.

Остановить конкретный зависимый срез при неизвестной host durability guarantee, неполном state inventory, ambiguous root без безопасного решения, отсутствии restore key или невозможности остановить writer. Записать precise blocker и продолжать независимые задачи. Вопрос производительности решать измерением; вопрос сохранности — корректным contract и доказательством, а не optimistic fallback.

## 9. Готовый промпт на одну задачу

```text
Выполни только S9-<NN> из docs/plans/STAGE_9.md.

1. Прочитай AGENTS.md, вложенные инструкции, PROJECT_LAYOUT.md,
   ARCHITECTURE.md, задачу и её реальные prerequisites. Проверь текущий diff.
2. Discovery начни с Graphify. Не считай scaffold/test fixture рабочей VM.
3. Укажи supported profile, normal/error outcomes, момент durable ack,
   владельцев данных, locks/leases и критерий независимой проверки.
4. Соблюдай роли моделей из AGENTS.md. Реализуй один рабочий срез:
   od-storage хранит формат/container; od-machine согласует CPU/I/O;
   od-host предоставляет host primitives. Tools не дублируют parser.
5. Проверяй latest durable watermark, referenced snapshots, partial/torn
   writes и потерю ответа после commit. Не делай silent rollback/zero fill.
6. Для полного checkpoint требуй всех настоящих participants и один epoch.
   Restore сначала validate/private stopped instance, потом activate.
7. Выполни нужные tests, отдельное ревью, исправь замечания, обнови Graphify
   после кода. Запиши факты в STAGE_9_REPORT.md и статус задачи;
   выявленные открытые проблемы — в KNOWN_ISSUES.md.
8. Не объявляй power-loss pass по kill процесса. Не выключай рабочий host
   и не форматируй носители без отдельного конкретного разрешения.

Единственный x86 — Mac mini Ubuntu, SSH senya@192.168.1.14.
Его Linux storage tests не доказывают durability macOS/APFS.
Большие образы и секреты не добавляй в Git.

Отчёт: что работает, файлы, checks/results, scope local/guest/hardware,
durability и identity ограничения, результат ревью, оставшиеся blockers.
Не отмечай весь этап завершённым по одному ограниченному срезу.
```

## 10. Проверка документа

Подготовка плана не является реализацией storage или проведённым экспериментом. Для текущей задачи проверяются scope roadmap/architecture/layout, последовательность и критерии задач, ссылки и Markdown. CPU/storage tests, guest запуск и SSH не требуются; commit содержит документацию.

2026-11-05 архитектурная роль `gpt-6-astra` / `high` проверила декомпозицию и границы: RAW-first, bounded CoW prototype, durable acknowledgement/recovery, полный rootset для reclamation, общий VM cut и UEFI/vTPM identity policy. Независимое ревью `gpt-6.1-sol` / `high` и повторная проверка выполнены. Уточнены зависимость управления от clone, удержание parent при copy-compaction и единоличное владение VM identity при restore/resume. Оставшихся замечаний к плану нет.

Проверки документа: 45 последовательных задач S9-00…44, известные IDs и отсутствие циклов в таблице зависимостей, наличие владельцев/критериев/чекбоксов, существующие локальные Markdown-ссылки и корректные code fences. `python3 tools/check_layout.py` и `git diff --check` прошли; новый файл отдельно проверен на завершающий перевод строки и отсутствие пробелов в конце строк. Эти результаты не являются приёмкой реализации этапа 9.
