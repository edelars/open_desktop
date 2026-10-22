# Карта проекта Open Desktop

Это нормативная карта размещения файлов. Общие правила — в [AGENTS.md](../AGENTS.md), архитектура — в [ARCHITECTURE.md](ARCHITECTURE.md), текущий план — в [plans/STAGE_0.md](plans/STAGE_0.md).

**Статус:** структура создана заранее по прямому поручению пользователя. Реализованы ограниченный CPU/JIT вертикальный срез, C ABI и smoke launcher ↔ VMHost; остальные каталоги остаются целевыми местами, а не подтверждёнными возможностями. Наличие каталога не означает готовность компонента и не расширяет задачу текущего этапа.

## Куда класть изменение

| Что добавляется | Единственное основное место |
|---|---|
| SwiftUI/AppKit, окна и библиотека VM | `apps/macos/Sources/` |
| Ресурсы и настройки подписи приложения | `apps/macos/Resources/`, `apps/macos/Configuration/` |
| Жизненный цикл VMHost, IPC transport, мост к Rust | `apps/vmhost/Sources/` |
| Настройки helper/подписи | `apps/vmhost/Configuration/` |
| CPU state, x86 decoder, interpreter | `crates/od-isa/src/` |
| IR, ARM64 emitter, dispatcher, JIT-кеш | `crates/od-jit/src/` |
| Гостевая RAM/MMU/TLB, vCPU и устройства | `crates/od-machine/src/` |
| Формат диска, метаданные и контейнер снимка | `crates/od-storage/src/` |
| macOS host-примитивы, JIT memory, glue C ABI | `crates/od-host/src/` |
| Общие генераторы/сравнение/сокращение тестов | `crates/od-testkit/src/` |
| Общий публичный host C ABI header | `include/open_desktop/` |
| Нормативный UI/launcher ↔ VMHost wire format | `protocols/control/` |
| Нормативный host ↔ guest wire format | `protocols/guest/` |
| Собственные машинные схемы, если необходимы | `protocols/schemas/` |
| Собственная UEFI и предоставляемые firmware-таблицы | `firmware/uefi/src/` |
| Userspace agent Windows/Linux | `guest/<os>/agent/src/` |
| Windows display/WDDM | `guest/windows/drivers/display/src/` |
| Прочие Windows drivers | `guest/windows/drivers/devices/src/` |
| Необходимые Linux kernel drivers | `guest/linux/drivers/src/` |
| Host headless runner | `tools/runner/src/` |
| Физический x86 oracle runner | `tools/x86-runner/src/` |
| Работа с дисковыми образами как инструмент | `tools/image-tool/src/` |
| Сборочная orchestration и генерация | `tools/build/` |
| Запуск и обработка benchmarks | `tools/bench/` |
| Межкомпонентные CPU/system/compat сценарии | `tests/cpu/`, `tests/system/`, `tests/compat/` |
| Небольшие собственные тестовые данные | `tests/fixtures/x86/`, `tests/fixtures/guest/` |
| Планы реализации | `docs/plans/` |
| Найденные недоделки и проблемы, не устранённые в ходе задачи | `docs/KNOWN_ISSUES.md` |
| Контракты CPU/платы/требований, обзоры ABI/протоколов | `docs/specs/` |
| Постоянные политики проекта | `docs/policies/` |
| Стенд и методики проверки | `docs/testing/` |
| ADR с обоснованием решений | `docs/decisions/` |
| Фактические результаты и необходимые сырые данные | `docs/reports/`, `docs/reports/data/` |
| Временные прогоны этапа 0 | `work/stage-0/<run-id>/` |
| Логи, scratch и локальные VM | `work/logs/`, `work/tmp/`, `work/vms/` |
| Готовые deliverables | `outputs/` |

Не копировать одну реализацию в несколько этих мест. Например, disk parser — в `od-storage`, а `image-tool` лишь использует его; x86 interpreter — в `od-isa`, а `runner` управляет запуском.

### Неочевидные границы

- Гостевое устройство: модель и guest-visible registers — в `od-machine`; системный backend — в host-слое; guest driver — в `guest/`; wire contract — в `protocols/guest/`.
- Снимок: остановка CPU/I/O и сбор согласованного состояния — в `od-machine`; запись контейнера/дисковых данных — в `od-storage`.
- JIT: блоки и dispatcher — в `od-jit`; отображение исполняемой памяти через macOS — в `od-host`, предоставляемое через узкий интерфейс.
- `docs/specs/HOST_ABI_V0.md` объясняет владение и поведение, а `include/open_desktop/` содержит общий C header. Документация не создаёт второй независимый набор объявлений.
- `docs/specs/CONTROL_PROTOCOL_V0.md` — обзор/границы; конкретные байты wire format нормативно описываются в `protocols/control/`. Связывать документы ссылками.
- Unit tests рядом с реализацией; общие сценарии — в `tests/`. Cargo сам не запускает произвольные файлы из корневого `tests/`: будущий harness подключается явно.

## Целевые зависимости

Стрелка означает «использует». Это разрешённая будущая схема, а не перечень уже подключённых dependencies.

```text
apps/macos --> control protocol --> apps/vmhost
apps/vmhost --> public C ABI --> od-host
od-host --> od-machine / od-jit / od-storage / od-isa
od-machine --> od-isa / od-jit / od-storage
od-jit --> od-isa
od-storage --> стандартные библиотеки / внедрённый storage I/O contract
od-testkit --> od-isa; прочие компоненты только для проверок
tools --> необходимые библиотеки
```

Нижние библиотеки не импортируют `od-host` ради системных ресурсов. Они получают необходимые узкие интерфейсы/handles, а верхний host-слой предоставляет реализацию. Это предотвращает цикл между machine/JIT и macOS host.

Production runtime не зависит от `od-testkit`. Библиотеки общего назначения разрешены во всех слоях по [политике зависимостей](policies/DEPENDENCIES.md) с учётом target/runtime; готовые ключевые компоненты движка запрещены. Firmware и guest-компоненты собираются отдельно и не входят в host workspace.

Внутренние path dependencies добавляются вместе с кодом, которому они нужны. Срез S0-09 использует `od-jit → od-isa`, `od-host → od-isa / od-jit`, а headless `tools/runner` вызывает `od-host / od-isa`; runner также использует `od-jit` напрямую только для измерения перевода без публикации кода. `od-jit` не зависит от host: исполняемую память предоставляет верхний `od-host` через свою границу исполнения. Остальные crate остаются независимыми до появления реального кода.

## Правила расширения

1. Выбрать владельца по таблице, прочитать вложенные инструкции.
2. Добавлять модули в существующий компонент прежде, чем создавать новую библиотеку.
3. Новый top-level каталог/crate требует объяснения ответственности, обновления этой карты, соответствующих инструкций и manifest при необходимости. Дополнительное подтверждение пользователя для обычной обоснованной правки структуры не требуется.
4. Не создавать root `src/`, `common/`, `utils/`, `shared/` и дублирующие приложения.
5. Удалять `.gitkeep` после появления в каталоге реальных отслеживаемых файлов. Не создавать фальшивые manifests, headers, CLI success responses и фиктивные тесты для заполнения каталогов.
6. Имена исходников и интерфейсов — английские. Windows/firmware code не маскировать под host Rust crate.
7. Проверить карту инструментом `python3 tools/check_layout.py`. Он проверяет размещение и локальные ссылки, но не обеспечивает корректность CPU и не заменяет чтение инструкций.

## Машиночитаемый перечень обязательных каталогов

Этот блок — единственный перечень каталогов для `tools/check_layout.py`. Родительские каталоги подразумеваются. Их наличие поддерживается исходниками, инструкциями или `.gitkeep`.

<!-- required-directories:start -->
```text
apps/macos/Sources
apps/macos/Resources
apps/macos/Configuration
apps/macos/Tests
apps/vmhost/Sources
apps/vmhost/Configuration
apps/vmhost/Tests
crates/od-isa/src
crates/od-jit/src
crates/od-machine/src
crates/od-storage/src
crates/od-host/src
crates/od-testkit/src
include/open_desktop
protocols/control
protocols/guest
protocols/schemas
firmware/uefi/src
firmware/uefi/include
firmware/uefi/tests
guest/windows/agent/src
guest/windows/drivers/display/src
guest/windows/drivers/devices/src
guest/windows/tests
guest/linux/agent/src
guest/linux/drivers/src
guest/linux/tests
tools/runner/src
tools/x86-runner/src
tools/image-tool/src
tools/build
tools/bench
tests/cpu
tests/system
tests/compat
tests/fixtures/x86
tests/fixtures/guest
docs/plans
docs/specs
docs/policies
docs/testing
docs/decisions
docs/reports/data
work/stage-0
work/logs
work/tmp
work/vms
outputs
```
<!-- required-directories:end -->

## Разрешённые необязательные каталоги в корне

Эти пользовательские каталоги могут присутствовать локально и не входят в обязательную структуру проекта. `iso/` содержит локальные образы и не версионируется; `.codex/` и `.opencode/` содержат локальную конфигурацию инструментов; `graphify-out/` — производный граф проекта.

<!-- allowed-root-directories:start -->
```text
.codex
.opencode
graphify-out
iso
```
<!-- allowed-root-directories:end -->

## Разрешённые исходные файлы в корне

При добавлении нового корневого файла обновить этот список. `.DS_Store` — системный файл Finder и не считается компонентом проекта. `Cargo.lock` версионируется. Git metadata и генерируемые `target/`, `build/`, `dist/` не являются исходными компонентами.

<!-- root-files:start -->
```text
AGENTS.md
README.md
Cargo.toml
Cargo.lock
.gitignore
.gitattributes
.editorconfig
```
<!-- root-files:end -->

## Проверки и статус

```sh
python3 tools/check_layout.py
cargo fmt --all -- --check
cargo check --workspace --all-targets --offline
```

Эти проверки подтверждают размещение файлов, форматирование и сборку существующих Rust-компонентов. Полный запуск приложения, Windows VM и готовность этапа 0 требуют отдельных критериев из [плана S0](plans/STAGE_0.md).

Корневой `AGENTS.md` обязателен; дополнительные инструкции находятся в каждом верхнеуровневом компоненте и отдельно в `guest/windows/` и `guest/linux/`. Пустые каталоги инструментов и приложений сохранены через `.gitkeep`.

Исходники, инструкции и небольшие доказательства версионируются. `work/`, `outputs/` и build outputs игнорируются, кроме их инструкций/маркеров. Временные и пользовательские файлы нельзя удалять только потому, что они игнорируются Git.
