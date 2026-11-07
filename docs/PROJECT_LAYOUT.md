# Карта проекта Open Desktop

Это нормативная карта размещения файлов. Общие правила — в [AGENTS.md](../AGENTS.md), архитектура — в [ARCHITECTURE.md](ARCHITECTURE.md), текущий план — в [plans/STAGE_2.md](plans/STAGE_2.md).

**Статус:** структура создана заранее по прямому поручению пользователя. Реализованы ограниченный CPU/JIT вертикальный срез, C ABI и smoke launcher ↔ VMHost; остальные каталоги остаются целевыми местами, а не подтверждёнными возможностями. Наличие каталога не означает готовность компонента и не расширяет задачу текущего этапа.

В этапе 2 принят MOV/v2 interpreter срез S2-08 и физически проверен
положительный memory oracle S2-09 (три programs ×2 и legacy 1010 cases ×2
на одном Intel CPU); методика и точные
границы — в [INTERPRETER](testing/INTERPRETER.md), фактические результаты —
в [отчёте](reports/STAGE_2_REPORT.md). Собственные исполняемые программы
нового среза находятся в [tests/fixtures/x86/stage2/](../tests/fixtures/x86/stage2/).
В S2-10 дополнительно проверены пять transfer программ (всего восемь v2 cases ×2)
и legacy 1010 cases ×2; [источники и evidence](reports/STAGE_2_REPORT.md#s2-10--transfer-extensions).
Это не подтверждает полный CPU или остальные memory/privileged формы.
S2-11 добавил arithmetic/eager flags: 313 новых cases, всего 321 v2 case ×2
и legacy 1010×2 прошли на одном Intel; [результат](reports/STAGE_2_REPORT.md#s2-11--арифметика-и-eager-flags)
отделяет положительный physical профиль от injected local fault checks.
S2-12 logic/TEST/NOT и semantic flags masks проверены в 197 новых cases,
всего 518×2; [результат](reports/STAGE_2_REPORT.md#s2-12--logic-testnot-и-semantic-definedness)
не объявляет полный CPU или аппаратную доставку guest #PF.
S2-13 shifts/rotates добавил 144 physical fixture inputs (142 register,
2 memory RMW); S2-14 добавил 38 успешных multiply/divide cases. Итоговый
корпус S2-09…14 прошёл 700 v2 inputs ×2 на Intel, без interpreter/native
различий; faulting divide cases проверены локальными unit tests и не входят
в physical fixtures. [S2-14 report](reports/STAGE_2_REPORT.md#s2-14--умножение-и-деление)
и [evidence](reports/data/stage2/multiply-divide-2026-10-22.json) фиксируют
результат и границу claim: physical `#DE` delivery/signal mapping не проверены.

Последовательные локальные S2-37av/aw/ax добавили interpreter-only register
profiles DIVPD, DIVPS и DIVSD; S2-37ax изменяет только low64 destination и
сохраняет high64. Оракулы и ограничения перечислены в [Stage 2 report](reports/STAGE_2_REPORT.md#s2-37ax--divsd-register-pair-interpreter-profile);
это локальные schema/interpreter checks, а не физическая приемка или поддержка
memory forms. S2-37be добавил CVTSD2SS: обновляется только destination.low32, high96
сохраняются; MXCSR rounding/FTZ и атомарные SIMD исключения ограничены
отдельным schema-v2 профилем.

## Куда класть изменение

| Что добавляется | Единственное основное место |
|---|---|
| SwiftUI/AppKit, окна и библиотека VM | `apps/macos/Sources/` |
| Ресурсы и настройки подписи приложения | `apps/macos/Resources/`, `apps/macos/Configuration/` |
| Общая Swift-библиотека приложения и CLI (цель `ODShared`, без `libod_host.a`; решение [ADR-0006](decisions/ADR-0006-desktop-app-shell.md)) | `apps/macos/Sources/Shared/` |
| Манифест Swift Package приложения, VMHost и CLI | `apps/Package.swift` |
| C-цель атомных функций транспорта кадров (`ODAtomics`) | `apps/macos/Sources/ODAtomics/` |
| C-цель объявлений C ABI движка для VMHost (`ODHostABI`) | `apps/vmhost/Sources/ODHostABI/` |
| CLI управления машинами (`odctl`) | `apps/cli/Sources/` (создаётся в S11-13) |
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
| Небольшие собственные тестовые данные | `tests/fixtures/x86/`, `tests/fixtures/guest/` (в том числе `tests/fixtures/guest/stage10/` для дисплея этапа 10) |
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

Ограниченный privileged LGDT oracle protocol S2-41a размещён в
[docs/testing/S2_41A_LGDT_ORACLE.md](testing/S2_41A_LGDT_ORACLE.md); это методика
будущей проверки, не реализованный harness и не physical evidence.

Предлагаемый privileged LIDT oracle protocol S2-41b размещён в
[docs/testing/S2_41B_LIDT_ORACLE.md](testing/S2_41B_LIDT_ORACLE.md); это
методика будущей проверки с открытыми harness/source-review prerequisites,
не исполняемый harness, fixture или physical evidence.

Ограниченный successful CPL3 FXSAVE64 oracle метод S2-39j и отдельный Linux
x86_64 harness S2-39k размещены в
[docs/testing/S2_39J_FXSAVE64_ORACLE.md](testing/S2_39J_FXSAVE64_ORACLE.md) и
[docs/testing/S2_39K_FXSAVE64_ORACLE.md](testing/S2_39K_FXSAVE64_ORACLE.md).
Отдельный результат Linux runtime/ptrace acceptance 2026-11-04 записан в
[docs/reports/S2_39K_LINUX_ACCEPTANCE.md](reports/S2_39K_LINUX_ACCEPTANCE.md);
raw evidence остаётся в ignored `work/stage2/s2-39k-acceptance-20261104-01/`.
Harness не меняет native fixture admission. Linux native A/B/A/B прошёл;
raw `execution_kind=unknown`, отдельная physical acceptance остаётся открытой.

Документы этапа 10 ([план](plans/STAGE_10.md)): контракт дисплейного устройства `specs/DISPLAY_DEVICE.md`, host ABI `specs/DISPLAY_HOST_ABI_V1.md`, транспорт кадров `../protocols/control/FRAME_TRANSPORT_V1.md`, протокол display-only драйвера `../protocols/guest/DISPLAY_DRIVER_V1.md`, host C ABI `../include/open_desktop/display_abi_v1.h`, транспорт ввода `../protocols/control/INPUT_TRANSPORT_V1.md`, методика `testing/DISPLAY.md`, отчёт `reports/STAGE_10_REPORT.md`. Файлы, которых ещё нет, создаёт соответствующая задача S10-NN; пока файла нет, его описание в плане не означает реализацию.

Документы этапа 11 ([план](plans/STAGE_11.md)): отчёт `reports/STAGE_11_REPORT.md`, решение об оболочке приложения `decisions/ADR-0006-desktop-app-shell.md`; далее по задачам — методика `testing/DESKTOP_APP.md`, контракт bundle `specs/VM_BUNDLE.md`, control v2 `../protocols/control/CONTROL_PROTOCOL_V2.md`, storage ABI `specs/STORAGE_HOST_ABI_V1.md` и `../include/open_desktop/storage_abi_v1.h`, каталог ошибок `specs/USER_FACING_ERRORS.md`, CLI `specs/CLI.md`, ADR транспорта агента `decisions/ADR-0007-agent-channel-transport.md` (создаётся в S11-20) и протокол агента `../protocols/guest/AGENT_PROTOCOL_V1.md`, устройство и ABI агента `specs/AGENT_CHANNEL_DEVICE.md`, `specs/AGENT_HOST_ABI_V1.md`, `../include/open_desktop/agent_abi_v1.h`, общие папки `specs/SHARED_FOLDERS.md`, сеть `decisions/ADR-0008-network-backend.md` и `specs/NETWORK.md`, `specs/NIC_DEVICE.md`, `specs/AUDIO_DEVICE.md`, scope-документы `specs/GUEST_WINDOW_INTEGRATION_SCOPE.md`, `specs/USB_PRINT_SCOPE.md`. Каталоги `apps/cli/` и `tests/fixtures/guest/stage11/` создают S11-13 и S11-23 и тогда же добавляют их в машиночитаемый перечень ниже. Файлы, которых ещё нет, создаёт соответствующая задача S11-NN; пока файла нет, его описание в плане не означает реализацию.

Не копировать одну реализацию в несколько этих мест. Например, disk parser — в `od-storage`, а `image-tool` лишь использует его; x86 interpreter — в `od-isa`, а `runner` управляет запуском.

### Неочевидные границы

- Гостевое устройство: модель и guest-visible registers — в `od-machine`; системный backend — в host-слое; guest driver — в `guest/`; wire contract — в `protocols/guest/`.
- Снимок: остановка CPU/I/O и сбор согласованного состояния — в `od-machine`; запись контейнера/дисковых данных — в `od-storage`.
- JIT: блоки и dispatcher — в `od-jit`; отображение исполняемой памяти через macOS — в `od-host`, предоставляемое через узкий интерфейс.
- Production memory/JIT boundary: `od-machine` owns RAM backing, guest mappings and MMU translation/permissions; `od-jit` owns fetch/decode-to-artifact policy, cache/dispatch and code-dependency tracking; `od-host` publishes and releases executable mappings. Writer notification is delivered to the JIT before the next lookup. This is the target dependency map, not a claim that production owner integration exists today; current borrowed memory dispatch is fixture-bounded.
- `docs/specs/HOST_ABI_V0.md` объясняет владение и поведение, а `include/open_desktop/` содержит общий C header. Документация не создаёт второй независимый набор объявлений.
- Opt-in session ABI v1: `docs/specs/HOST_SESSION_ABI_V1.md` задаёт отдельный контракт владения/вызовов, `include/open_desktop/session_abi_v1.h` — его C-объявления. Существующий `host_abi.h` сохраняет ABI v0.
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
apps/macos/Sources/Shared
apps/macos/Sources/ODAtomics/include
apps/cli/Sources/odctl
apps/vmhost/Sources
apps/vmhost/Sources/ODHostABI/include
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
CLAUDE.md
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

Эти проверки подтверждают размещение файлов, форматирование и сборку существующих Rust-компонентов. Полный CPU, запуск гостевых ОС и готовность этапа 2 требуют отдельных критериев из [плана S2](plans/STAGE_2.md); незакрытые условия этапа 0 сохраняются в [плане S0](plans/STAGE_0.md).

Корневой `AGENTS.md` содержит общие правила проекта; `CLAUDE.md` направляет Claude к ним и уточняет выбор ролей для Claude. Дополнительные инструкции находятся в каждом верхнеуровневом компоненте и отдельно в `guest/windows/` и `guest/linux/`. Пустые каталоги инструментов и приложений сохранены через `.gitkeep`.

Исходники, инструкции и небольшие доказательства версионируются. `work/`, `outputs/` и build outputs игнорируются, кроме их инструкций/маркеров. Временные и пользовательские файлы нельзя удалять только потому, что они игнорируются Git.
