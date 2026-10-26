# Этап 3. Первый JIT x86 → ARM64 — пошаговый план реализации

**Дата:** 26 октября 2026 года. **Статус:** этап начат; S3-00, S3-02, S3-03, S3-04, S3-05 и S3-06 завершены; S3-01 в работе.

План предназначен для модели, которой нужны небольшие последовательные задания, конкретные файлы, зависимости и проверяемый результат. Выполнять одну задачу `S3-NN` за раз. Не реализовывать весь JIT одним изменением. Большие группы внутри задачи проходить по указанным подшагам.

Нормативный объём — [этап 3 дорожной карты](../ROADMAP.md#этап-3-первый-jit-x86--arm64). Границы компонентов определяют [карта проекта](../PROJECT_LAYOUT.md) и [архитектура](../ARCHITECTURE.md). Семантика x86 принадлежит [CPU contract](../specs/CPU_CONTRACT.md) и [интерпретатору этапа 2](STAGE_2.md). Профиль первого среза зафиксирован в [контракте JIT](../specs/JIT_CONTRACT.md), методика — в [проверках JIT](../testing/JIT.md). Этот план описывает перевод и исполнение уже определённой семантики, а не второй CPU-профиль.

## 1. Проверяемый результат этапа

Собственный JIT переводит поддержанное подмножество x86 в ARM64, повторно использует опубликованные блоки, передаёт управление между ними и собственному интерпретатору, сохраняет точное состояние на выходах и не исполняет устаревший код после его изменения.

```text
guest RIP + CPU context + memory/code versions
  → поиск пригодного блока
  → при промахе: fetch/decode → свой IR → ARM64 → безопасная публикация
  → исполнение блока
  → следующий блок / interpreter step / точный выход
  → материализация состояния и проверка по interpreter/native x86
```

Порядок проверяемых срезов:

1. **A:** один блок для уже проверенного register-only профиля, свой IR, ARM64, eager flags, точный snapshot.
2. **B:** dispatcher, несколько блоков, ограниченный cache, корректный interpreter fallback и STOP/budget.
3. **C:** поддержанные операции памяти, faults, инвалидация изменённого кода и безопасное освобождение блоков.
4. **D:** вычисление flags по потребности, улучшения register allocation и другие подтверждённые замерами изменения, интеграция и итоговая приёмка.

Для каждого среза записывать exact instruction forms, режим, состояние, memory capabilities и внешние зависимости. Не называть исполнение через interpreter нативным JIT lowering. Успешная сборка или совпадение только двух собственных движков не закрывает физическую проверку.

## 2. Исходное состояние и зависимости

Состояние зависимостей перепроверено 26 октября 2026 года:

- [Этап 1 принят](../reports/STAGE_1_REPORT.md) для ограниченного S0-09 register-only профиля. Есть физический oracle, fixtures, генератор, comparator, reducer и повторяемый harness.
- [Этап 2 выполняется](../reports/STAGE_2_REPORT.md): отчёт фиксирует принятые срезы на момент последней записи, но этап 2 в целом не принят и развивается параллельно. Проверки S3 привязывать к точному fingerprint; изменения этапа 2 не расширяют JIT-профиль автоматически.
- `crates/od-jit/src/lib.rs` содержит ограниченный прямой fragment translator и ARM64 emitter. Он не доказывает наличие общего IR, dispatcher, block cache или self-modifying-code support.
- `crates/od-host/src/lib.rs` содержит `ExecutableMemory`, `CompiledFragment`, macOS MAP_JIT path, переключение write protection и синхронизацию instruction cache. Эти механизмы нужно расширять с сохранением проверенных ограничений, а не писать второй независимый host allocator.
- Старый fragment state/C ABI и новый архитектурный state этапа 2 различаются. JIT layout использует `offset_of!`; перед новой реализацией проверить фактические типы, adapters, generated accesses и helper signatures.
- Имеются fixture v1 и новый v2 path. Текущий v2 JIT возвращает явный Unsupported; этот отказ не означает готовую JIT-поддержку расширенного state или памяти.
- В [протоколе измерений](../testing/BENCHMARKS.md) и [baseline S0-13](../reports/STAGE_0_CPU_BASELINE_2026-10-22.md) описан узкий fragment benchmark. Старые числа не являются baseline будущего block JIT на новых программах.

**Основной host:** M3 Max, macOS/ARM64. **Дополнительный ARM64 host:** M1 Pro в пределах доступной конфигурации. **Единственный физический x86 oracle:** Mac mini с Ubuntu, SSH `senya@192.168.1.14`; в этапе 1 использован checkout `/home/senya/projects/open_desktop-stage1`. Актуальные CPU/OS/toolchain/page sizes брать из [STAND.md](../testing/STAND.md) и перепроверять перед прогонами. Старый remote checkout не считать синхронизированным автоматически.

Не требуется ждать завершения всей ISA этапа 2 для среза A. Но новую форму нельзя принять в JIT до подтверждения её interpreter semantics и независимой проверки. Для C обязательны S2-09 и соответствующие memory/fault cases; для REP/system/FP fallback нужны реально завершённые соответствующие задачи этапа 2. Если зависимости нет, продолжать независимую задачу, а зависимую оставить открытой.

Известные ограничения находятся в [KNOWN_ISSUES](../KNOWN_ISSUES.md): ISSUE-004 — поставка/подпись, ISSUE-010 — неполное извлечение C header графом, ISSUE-011 — проверка источника AMD, ISSUE-012 — неподтверждённая граница operand range через верх u64. Не переопределять эти условия внутри JIT и не записывать их закрытыми по результату этого плана.

## 3. Правила для исполнителя

1. Перед задачей прочитать корневой `AGENTS.md`, этот план, карту проекта, применимые `crates/`, `tools/`, `tests/`, `apps/` инструкции и вложенные `AGENTS.md` целевых файлов.
2. Проверить `git status --short`. В дереве могут выполняться задачи этапа 2; не перезаписывать их и не откатывать ради чистого baseline. Для доказательств сохранять revision и fingerprint фактически проверенных исходников.
3. При наличии графа выполнить `graphify query "<вопрос текущей задачи>"` до поиска/чтения исходников. Ограничение графа по ABI учитывать: найденный реальный C header и его consumers нужно проверить непосредственно.
4. Выбрать ближайшую незавершённую задачу с готовыми зависимостями. Сначала сформулировать вход, ожидаемое состояние/выход и негативный случай; затем реализовать минимальный путь.
5. Использовать собственные IR, emitter, allocator и runtime. Не подключать LLVM, Cranelift, готовый DBT/JIT или чужие x86 decoder/semantics. Общая инфраструктура разрешена по [политике зависимостей](../policies/DEPENDENCIES.md).
6. Нормативные источники фиксировать с версиями: x86 semantics — Intel/AMD и CPU contract; ARM64 encoding/ABI — официальные Arm и Apple документы; executable memory — выбранный macOS API contract. Не выводить ABI из случайного успешного запуска.
7. Производительность измерять до окончательного выбора критичного решения. Простой корректный вариант допустим как baseline; его скорость не считается доказанной. Для нового горячего участка сравнить минимальные варианты на одинаковой нагрузке.
8. Не добавлять background compiler, SMP cache protocol, глобальный SSA, persistent code cache или универсальную plugin-систему без конкретного требования. Первый runtime — один vCPU, последовательная компиляция и явные безопасные точки.
9. Guest fault, unsupported form, compiler failure, allocation failure, host crash, budget и STOP различать. Не превращать внутренний сбой в guest exception или успешный fallback.
10. После задачи выполнить её проверки и общие проверки раздела 11. После изменения кода — `graphify update .`. Обновить документацию и реальные найденные проблемы.
11. `[x]` означает работающую реализацию с доказательствами. Не закрывать JIT-группу только тем, что её целиком исполнил interpreter. Не закрывать hardware suite при skip/недоступном SSH.
12. В отчёте указать ID, файлы, поддержанные формы, проверки, измерения при наличии, ограничения и следующий ID. Этап закрывается только S3-41.

## 4. Размещение и обязательные контракты

### 4.1. Где менять код

| Ответственность | Место |
|---|---|
| IR, block builder, lowering, ARM64 encoding, register allocation | `crates/od-jit/src/` |
| Dispatcher, block lookup, metadata, зависимости от code pages | `crates/od-jit/src/` |
| macOS executable mappings, write protection, I-cache sync, host resources | `crates/od-host/src/` |
| CPU state, decoder, step interpreter, архитектурные exceptions | существующий `crates/od-isa/src/` |
| Guest translation, RAM/MMIO, mapping changes и реальные device writers | `crates/od-machine/`, по мере этапа 4 |
| Fixture memory, generation/comparison/reduction | существующий `crates/od-testkit/src/` |
| CLI, сбор сравнения/benchmarks | существующий `tools/runner/`, `tools/bench/` |
| CPU/JIT scenarios | `tests/cpu/`; host/process scenarios — `tests/system/` |
| Собственные малые входы | предлагаемый `tests/fixtures/x86/stage3/` |
| Контракт JIT | будущий `docs/specs/JIT_CONTRACT.md` |
| Методика | будущий `docs/testing/JIT.md`, существующий BENCHMARKS |
| Факты и замеры | будущий `docs/reports/STAGE_3_REPORT.md`, `docs/reports/data/stage3/` |
| Временные данные | `work/tmp/stage3/<run-id>/` |

Новые имена — предлагаемые пути, не существующие реализации. Создавать модули `ir.rs`, `block.rs`, `arm64/`, `regalloc.rs`, `runtime.rs`, `cache.rs`, `state_map.rs` внутри `od-jit` только при появлении соответствующего кода. Не дробить проект на новые crates заранее.

`od-jit` не импортирует `od-host`. Он получает нужный узкий интерфейс публикации/владения executable code; реализацию macOS предоставляет верхний слой. `od-machine` предоставляет guest memory/context, а не переносит MMU в JIT. Runtime не зависит от `od-testkit`. Существующий публичный C ABI не расширять молча.

### 4.2. Выполнение и состояние

- Один canonical архитектурный state на границе исполнения; layout generated code и helpers задан явно и проверен. Host-register cache значений не становится новым архитектурным состоянием.
- На выходе доступны правильные GPR, flags, RIP, memory effects, exit reason и согласованный instruction count. Host/instrumentation instructions в guest count не входят.
- Любой вызов helper имеет объявленные reads/writes/clobbers, fault behavior и точку commit. До callback материализуются необходимые значения; после callback инвалидируются затронутые host-register bindings.
- Соблюдать ARM64/macOS calling convention, stack alignment, callee-saved и platform-reserved registers. Никакой Rust unwind через generated code. Host panic остаётся внутренней фатальной ошибкой, а не guest fault.
- Не держать конфликтующие Rust references на CPU/RAM одновременно с доступом generated code или callback. Владение и lifetime проверяются на границе, а не объясняются одним `unsafe` блоком.
- Fallback вызывает существующий interpreter на точном текущем RIP и с общим budget/event context. Он должен продвинуть выполнение либо вернуть явный выход; бесконечный цикл «compile unsupported → retry того же блока» запрещён.

### 4.3. Блок и его зависимости

Блок содержит entry context, последовательность guest instructions, exit edges, guest-PC/host-PC metadata, необходимые code dependencies и executable handle. Границы: control transfer, неподдерживаемая форма, необходимый serializing/context-changing effect, лимит размера и безопасная точка. Максимальные guest instructions/bytes и host code size задаются явно.

Compiler не исполняет гостя. Ошибка fetch следующей инструкции при построении блока не должна возникать раньше, чем interpreter дошёл бы до неё: закончить уже построенный валидный префикс, а fault получить при достижении faulting RIP. То же относится к unsupported/invalid байтам после ещё не исполненных инструкций. Не читать всю будущую страницу ради удобства decoder.

Cache key учитывает entry RIP и действительно значимый decode/execution context: профиль/режим, relevant sizes/privilege, identity/version адресного пространства и прочие специализации. Конкретный набор записать в JIT_CONTRACT. Не включать весь CpuState без причины и не ограничиваться одним RIP. Constant assumptions обязаны иметь guard/version dependency.

Code dependencies покрывают все затронутые гостевые страницы/регионы и mapping identities. Host page size и guest page size могут отличаться. Guest virtual aliases одной памяти должны инвалидировать один и тот же зависимый перевод. Для flat fixtures разрешён простой region generation; это ещё не production physical-page tracking.

### 4.4. Публикация и освобождение

Сначала сформировать и проверить bytes/relocations/metadata, затем записать код через выбранный host API, выполнить требуемую синхронизацию и только после этого сделать entry доступным runtime. Для публикации проверить, что версии исходного кода ещё актуальны.

Первый вариант публикует immutable blocks. Изменённый перевод публиковать как новый block, старый выводить из lookup и освобождать после отсутствия активного исполнения/ссылок. Не patch-ить исполняемый блок на месте без отдельного доказанного протокола. Eviction и invalidation не имеют права уничтожить код под текущим PC.

Отдельно проверить allocation failure, публикацию после invalidation, повторный destroy, освобождение после fault и исчерпание cache capacity. Для одного vCPU достаточно простого явного active-block/retired-block lifecycle; SMP reclamation относится к последующему этапу.

### 4.5. Изменение гостевого кода

Учитывать записи interpreter, JIT, debugger/loader и будущих device/DMA writers через общую границу уведомления memory owner. Страница не обязана быть read-only, чтобы содержать уже переведённый код.

Инвалидации cache entry недостаточно, если запись изменила ещё не исполненные инструкции текущего блока. Для первой корректной реализации после записи в зависимый код немедленно выйти на безопасной границе и продолжить с правильного guest RIP через повторную проверку/перевод. Не продолжать stale host instructions. Оптимизировать этот случай только после измерений и сохранения семантики.

Host PC map нужен для диагностики и восстановления на предусмотренных выходах. Он не превращает произвольный SIGSEGV/SIGBUS generated code в корректное гостевое исключение. Первый memory path использует явные checked helpers; неожиданный host fault диагностируется как ошибка VMHost.

## 5. Порядок и статусы задач

Внешние зависимости S2 относятся к [плану этапа 2](STAGE_2.md). Готовность перепроверяется перед исполнением задачи. Условные оптимизации можно оставить без внедрения с результатами сравнения; обязательные функции roadmap нельзя молча объявлять необязательными.

| Статус | ID | Задача | Зависимости |
|---|---|---|---|
| [x] | S3-00 | Зафиксировать поддержанное подмножество и источники | Принятый профиль этапа 1; фактический статус S2 |
| [ ] | S3-01 | Baseline и протокол сравнения вариантов | S3-00 |
| [x] | S3-02 | Runtime/state/helper contract | S3-00 |
| [x] | S3-03 | Типизированный IR и verifier | S3-02 |
| [x] | S3-04 | Построение bounded basic block | S3-03 |
| [x] | S3-05 | Перевод decoder output в IR | S3-04 |
| [x] | S3-06 | Проверяемый ARM64 encoder | S3-02 |
| [ ] | S3-07 | Executable publication и ownership | S3-02, S3-06 |
| [ ] | S3-08 | Generated entry и helper ABI | S3-07 |
| [ ] | S3-09 | Простой allocator baseline | S3-03, S3-08 |
| [ ] | S3-10 | Lowering первого integer subset | S3-05, S3-06, S3-09 |
| [ ] | S3-11 | Корректные eager flags | S3-10 |
| [ ] | S3-12 | State points и PC maps | S3-11 |
| [ ] | S3-13 | Первый сквозной запуск одного блока | S3-07, S3-12 |
| [ ] | S3-14 | Тройное сравнение register-only | S3-13 |
| [ ] | S3-15 | Dispatcher, budget и переходы | S3-14 |
| [ ] | S3-16 | Interpreter fallback с прогрессом | S3-15; нужный S2 step |
| [ ] | S3-17 | Cache key и code dependencies | S3-04, S3-15 |
| [ ] | S3-18 | Ограниченный block cache | S3-07, S3-17 |
| [ ] | S3-19 | Checked memory/helper bridge | S3-08, S3-16; S2-04/08 |
| [ ] | S3-20 | Memory lowering и partial registers | S3-19; interpreter соответствующих форм |
| [ ] | S3-21 | Физическая проверка memory slice | S3-20; S2-09 и нужный native profile |
| [ ] | S3-22 | Смена execution/mapping context | S3-18, S3-21 |
| [ ] | S3-23 | Инвалидация code dependencies | S3-18, S3-21, S3-22 |
| [ ] | S3-24 | Self-modifying code в текущем блоке | S3-23 |
| [ ] | S3-25 | Eviction, retirement и освобождение | S3-18, S3-24 |
| [ ] | S3-26 | Порядок side effects и memory classes | S3-19, S3-24 |
| [ ] | S3-27 | Точные fault exits и восстановление state | S3-12, S3-21, S3-26 |
| [ ] | S3-28 | STOP/events и безопасные точки | S3-15, S3-16, S3-27 |
| [ ] | S3-29 | Flags по потребности и сравнение стратегий | S3-01, S3-27, S3-28 |
| [ ] | S3-30 | Выбор allocator по измерениям | S3-01, S3-29 |
| [ ] | S3-31 | Прямые связи блоков, если оправданы | S3-25, S3-28, S3-30 |
| [ ] | S3-32 | Локальные IR-преобразования, если оправданы | S3-03, S3-27, S3-30 |
| [ ] | S3-33 | Расширение lowering по готовности S2 | S3-27, S3-29; готовый interpreter/native subset |
| [ ] | S3-34 | Corpus, reducer и негативные проверки | S3-24–S3-33 в принятой конфигурации |
| [ ] | S3-35 | Измерить холодное исполнение | S3-01, S3-34 |
| [ ] | S3-36 | Измерить прогретое исполнение | S3-35 |
| [ ] | S3-37 | Cache churn и ресурсы | S3-25, S3-34 |
| [ ] | S3-38 | Интеграция VMHost и ABI | S3-28, S3-34 |
| [ ] | S3-39 | Повторить macOS/ARM smoke на M1 Pro | S3-38 |
| [ ] | S3-40 | Зафиксировать решения по данным | S3-35–S3-39 |
| [ ] | S3-41 | Документация и приёмка этапа | Все обязательные задачи |

Первый вертикальный путь: `00 → 02 → 03 → 04 → 05`, затем `06 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 14`. Baseline S3-01 выполняется до выбора оптимизаций. Тесты добавляются в каждой задаче, S3-34 объединяет их и проверяет сам стенд.

## 6. Первый блок: от IR до физического сравнения

### S3-00. Зафиксировать область первого JIT

**Файлы:** [JIT_CONTRACT](../specs/JIT_CONTRACT.md), [JIT testing](../testing/JIT.md); ссылки на CPU_CONTRACT и текущие отчёты.

1. Сопоставить реальные формы interpreter/JIT/native. Начать с подтверждённого S0 register subset; расширенный v2/memory профиль вести отдельными строками.
2. Для каждой строки указать native lowering, helper, interpreter fallback или unsupported, область сравнения и внешний prerequisite.
3. Зафиксировать один vCPU, последовательную компиляцию, первые block limits и выбранную host-конфигурацию. Лимиты — явные начальные ограничения, а не утверждение об оптимальности.
4. Записать нормативные источники ARM64/ABI/macOS API и версии, нужные для конкретных используемых механизмов.

**Готово:** [контракт JIT](../specs/JIT_CONTRACT.md) фиксирует точные байтовые формы первого S0-09 subset, native/helper/unsupported границы, сравниваемое состояние, начальные runtime limits, host configuration и источники ISA/ABI/macOS API. [Методика](../testing/JIT.md) задаёт порядок локального и физического сравнения и prerequisites будущих срезов. Этап 2 не объявлен завершённым, а memory semantics не стала основанием JIT-приёмки.

### S3-01. Подготовить baseline и сравнение вариантов

**Файлы:** BENCHMARKS, существующий runner/bench harness; evidence после реальных запусков.

1. Запустить текущий local/oracle набор этапа 1 и сохранить fingerprint. Зафиксировать фактический статус S2 memory/native проверок.
2. Описать workloads: линейный integer, короткий/длинный loop, conditional branches, несколько blocks, helper-heavy путь; memory/fallback/invalidation добавлять вместе с их реализацией.
3. Разделить cold process start, cold code cache и warm block execution. Задать одинаковые входы, начальный state, guest work, correctness и build conditions.
4. Подготовить повторные calibrated samples с переменным порядком вариантов и сырыми durations/counts. Не придумывать speedup threshold без требований; шум не считать улучшением.

**Готово:** воспроизводимый протокол позволяет сравнивать решения до их окончательного выбора. Отсутствующие workloads отмечены, старые fragment numbers не перенесены на block JIT.

### S3-02. Определить runtime contract

**Файлы:** od-jit, od-host interface boundary, JIT_CONTRACT.

1. Описать runtime context, входы/выходы блока, state layout, budget, stop/event checks и helper ABI.
2. Развести guest fault, fallback request, unsupported, compilation failure и host failure. Зафиксировать, какие ошибки допускают документированный interpreter path, а какие прекращают запуск.
3. Описать executable handle, ownership и обязанности publisher. Не создавать циклическую зависимость `od-jit ↔ od-host`.

**Готово:** интерфейс покрывает конкретный первый срез и типы выходов; нет Rust enum/layout guesses в generated ABI и необъявленных clobbers.

### S3-03. Ввести небольшой типизированный IR

**Предлагаемые файлы:** `od-jit/src/ir.rs` и локальные tests.

1. Добавить только нужные операции: constants/temporaries, reads/writes guest state, integer effects, flags, branches, exits; memory/helper forms добавлять с их задачами.
2. Каждой операции задать width, inputs/outputs и side effects. Guest instruction boundary и commit point должны оставаться представимыми.
3. Реализовать verifier: определённость values, widths, допустимые targets, обязательные exits и state metadata. Не начинать с глобального SSA-оптимизатора.

**Готово:** некорректный IR отвергается до executable publication. Тесты проверяют реальные нарушения контракта, а не только существование enum.

### S3-04. Построить bounded basic block

**Файлы:** `od-jit/src/block.rs`, decoder adapter.

1. Декодировать с entry RIP по одному instruction, используя существующий decoder и mode context.
2. Останавливать построение на terminator, лимите, context-changing effect или форме без lowering. Не интерпретировать недостижимый хвост.
3. На fetch/decode fault после валидного префикса завершить префикс с выходом к следующему RIP. Fault на первой инструкции наблюдать в нужной runtime точке, не раньше исполнения предыдущего блока.
4. Сохранить guest byte ranges всех decoded instructions, включая переход через границу страницы. Code identity/version tracking относится к S3-17, проверка изменения и cache invalidation — к S3-23.

**Готово:** tests покрывают entry на новой boundary, entry на code_end, instruction/byte limits, truncation на первой инструкции и после валидного префикса, unsupported form после префикса, branch без декодирования хвоста, target внутрь инструкции, zero-displacement target на `next_rip`/`code_end` и unresolved forward target. Для декодированного префикса сохранены точные guest ranges; fetch exhaustion не назван guest fault. Mid-instruction target останавливает сборку до branch и сохраняет prefix. Branch target вычисляется 64-битным wrapping сложением, как в interpreter; `next_rip` текущей Jcc считается известной sequential boundary, но последующие bytes не декодируются. Остальные unresolved targets не передаются текущему IR verifier до решения S3-05/S3-15. Ranges не устанавливают code identity/version; tracking принадлежит S3-17, а cache invalidation — S3-23.

### S3-05. Перевести decoder output в IR

1. Реализовать MOV/ADD/SUB/CMP/JZ/JNZ exact forms первого профиля. Сохранить operand widths, flags effects и guest RIP каждой инструкции.
2. Неподдержанный lowering оформлять явным block exit/fallback request, не NOP и не приблизительным результатом.
3. Использовать decoder description; не декодировать x86 повторно в emitter и не копировать семантику в runner.
4. Для unresolved Jcc не включать branch в IR: сохранить prefix-only `NeedInterpreter` outcome, а для пустого prefix возвращать явный `NoNativePrefix`. Mid-instruction target должен завершаться `Unsupported` до Jcc.
5. Передавать точный `CodeView.code_end` в `Program`; verifier принимает только эту дополнительную boundary и проверяет, что guest records находятся внутри неё. Commit marker и terminal branch вместе образуют retirement/target-selection boundary без observable operation между ними.

**Готово:** IR содержит наблюдаемые effects каждого ручного case, verifier проходит; неверная или неподдержанная форма не публикуется как успешный native block. Targeted tests должны покрывать operand alias/order, effects, branch boundary cases, prefix fallback, deferred decode boundary и code_end admission.

### S3-06. Выделить и проверить ARM64 encoder

**Файлы:** `od-jit/src/arm64/encoder.rs`, узкий публичный модуль `arm64`, существующий caller `od-jit/src/lib.rs` и небольшой local assembler fixture.

1. Перенести существующие используемые encodings в typed fallible API без изменения legacy fragment output. Отдельно типизировать GPR, SP и zero registers; проверять immediate, scaled/signed offsets, alignment и little-endian serialization.
2. Поддержать текущие branch/call encodings и local-label relocations. `finish` не должен возвращать output при unresolved labels или displacement вне диапазона; displacement не обрезать молча.
3. Сверить небольшой набор слов с локальным Apple ARM64 assembler fixture и нормативной A64 reference; зафиксировать toolchain/target. Не добавлять executable-memory publication (S3-07), entry/helper ABI (S3-08), allocator (S3-09), lowering (S3-10), veneers/relaxation или общую ISA framework.

**Готово:** known encodings и range failures проверены независимо от собственного x86 decoder; в `od-jit` fragment translator сохраняется прежний word output. Несовместимая или неразрешённая encoding возвращает ошибку до выдачи буфера.

### S3-07. Сделать публикацию кода управляемой

**Файлы:** существующий executable-memory path od-host и узкий publisher interface.

1. Повторно использовать macOS JIT APIs и соответствующую конфигурацию подписи. Проверить min-OS/API availability и lifecycle на выбранных host версиях.
2. Разделить writable construction и executable publication, выполнить необходимую instruction-cache синхронизацию до entry visibility.
3. Возвращать owning handle; allocation/publication failure не возвращает usable entry. Проверить очистку частично созданного mapping.
4. Сначала immutable blocks; никаких конкурентных write/execute в одной области и patching активного кода.

**Готово:** generated smoke исполняется вне debugger в предусмотренной host-конфигурации; failures детерминированны, память освобождается. Это не закрывает ISSUE-004 о distribution signing.

### S3-08. Реализовать generated entry и helper calls

1. Зафиксировать prologue/epilogue, stack alignment, callee-saved/platform registers и representation arguments/results.
2. Создать один реальный helper call с объявленными clobbers; проверить state до/после, nested host call и error return.
3. Исключить unwind через generated code и aliasing живых Rust references с generated state access. Host failures не преобразовывать в guest exceptions.

**Готово:** ABI stress tests сохраняют host state и guest values; неверные helper outcomes не проходят как completed.

### S3-09. Реализовать простой allocator baseline

1. Для первого набора temporaries выбрать простой проверяемый mapping/spill strategy с ограниченным scratch register set.
2. Реализовать stack slots и загрузку/сохранение values вокруг helper calls; запрещённые host registers исключить явно.
3. Добавить проверки конфликтов live values и spill slot bounds. Считать spill/reload/code size для будущего сравнения.

**Готово:** искусственно тесный register budget не меняет результат. Это корректный исходный вариант; окончательный выбор горячего allocator делается по S3-30.

### S3-10. Выполнить первый integer lowering

1. Добавить native lowering constants, MOV и арифметического результата ADD/SUB/CMP первого профиля.
2. Правильно обрабатывать guest widths и host zero/sign extension; CMP не записывает arithmetic result в guest destination.
3. Проверить chains temporaries, spills и входные registers с крайними bit patterns.

**Готово:** реальные ARM64 bytes исполняются с правильными register results. Flags доводятся в S3-11; до этого instruction support не объявляется полностью готовым.

### S3-11. Реализовать eager flags baseline

1. Материализовать определённые CF/PF/AF/ZF/SF/OF для текущих форм, сохраняя требуемые неизменяемые bits.
2. Не отождествлять host NZCV с полным x86 RFLAGS. Jcc читает guest condition, а helper clobber не уничтожает pending значение.
3. Сверить carry/borrow, signed overflow, auxiliary carry и parity по независимым byte fixtures.

**Готово:** арифметика и taken/not-taken branches совпадают с interpreter; definedness masks заданы до наблюдения результата. Eager path остаётся эталонной конфигурацией для проверки S3-29.

### S3-12. Добавить state points и PC maps

1. Для каждой guest instruction и каждого предусмотренного exit записать guest RIP, host code range, commit state и способ материализации live values.
2. Восстановить точное состояние при helper/fallback/normal exits; не записывать next RIP до проверки faulting операции.
3. Проверить maps после relocation и связывание metadata с lifetime executable block. Padding/prologue/epilogue не выдавать за отдельные guest instructions.

**Готово:** диагностический host PC однозначно связан с нужным guest boundary либо служебным кодом; snapshots на planned exits точны. Произвольный host crash не заявляется восстанавливаемым guest fault.

### S3-13. Подключить один блок к CLI

**Файлы:** существующий od-runner, od-host/od-jit adapters, testkit result path.

1. Выполнить compile → publish → execute → snapshot для первого профиля с теми же входами и compare scope, что у interpreter.
2. Сохранить старый fragment entry/ABI и fixtures. Новый путь должен быть различим в metadata результата, без подмены исторических evidence.
3. Проверить completed, unsupported, allocation/compile error и malformed result paths.

**Готово:** одна документированная команда реально исполняет IR-based block и выдаёт machine-readable result. Нулевой код возврата не появляется при fallback failure или отсутствии lowering.

### S3-14. Подтвердить register-only срез физически

1. Составить явный manifest программ, целиком исполняемых одним блоком: линейные формы и разрешённые terminal branches. Использовать подходящие неизменённые fixtures этапа 1 и собственные новые случаи; не требовать ещё отсутствующий dispatcher.
2. Сравнить interpreter, новый JIT и Mac mini native по определённому состоянию; повторить каждый case минимум дважды. Завершение native программы должно совпадать с выбранной границей сравнения.
3. Сохранить input, snapshots, fingerprints, host/toolchain и failures. Включить ABI/spill/branch boundary cases нового pipeline. Полный multi-block corpus этапа 1 запускается после S3-15.

**Готово:** срез A подтверждён тройным сравнением. Неподдержанные обязательные cases, недоступный SSH и mismatch оставляют задачу открытой.

## 7. Dispatcher, cache и память

### S3-15. Реализовать dispatcher и точный budget

1. После блока обработать next RIP, conditional/indirect target и выход без recursion по длинной цепочке блоков.
2. Считать guest instructions в JIT и fallback по единому contract. При недостаточном остатке budget использовать точный ограниченный путь, а не исполнять весь блок сверх лимита.
3. Проверить loop, zero/one budget, выход посреди блока, сохранение flags и bounded stack usage dispatcher.
4. Теперь прогнать все 10 ручных cases и фиксированный corpus из 1000 generated cases этапа 1 через новый runtime, interpreter и native oracle, минимум два повтора. Старые inputs/digests сохранить; сравнение только одного блока из программы больше не является достаточным.

**Готово:** multi-block program даёт состояние interpreter при том же budget; ошибочный target или невозможный exit не зацикливает runtime.

### S3-16. Подключить interpreter fallback

1. На форме без lowering материализовать state и вызвать реально поддерживающий её interpreter step. При отсутствии поддержки обоих вернуть unsupported.
2. Согласовать budget, STOP, fault и memory effects. После helper обновить context/code validity перед возвратом в JIT.
3. Защититься от нулевого прогресса и повторной компиляции того же неподдержанного entry без исполнения.
4. Учитывать compiled instructions, helper calls и interpreter steps раздельно в metadata/benchmarks.

**Готово:** смесь native blocks и interpreter instructions совпадает с чистым interpreter. Новые S2 формы используются только после их проверки; all-fallback workload не называется JIT-ускорением.

### S3-17. Определить cache key и зависимости

1. Записать minimum relevant context key по разделу 4.3; включить все реальные compile-time assumptions и версии code/mappings.
2. Собрать зависимости blocks, пересекающих несколько regions/pages. Определить identity для aliases и повторного использования адреса памяти.
3. Проверить разные bytes/context при одинаковом RIP и одинаковые bytes при изменённой permissions/mapping identity.

**Готово:** stale или несовместимый block не считается hit. Выбор структуры поиска предварительно сравнен на representative key/workload; простой вариант допустим как измеренный baseline.

### S3-18. Реализовать ограниченный cache

1. Добавить lookup, compile miss, publish и reuse; проверить версии до вставки и входа в block.
2. Задать capacity по blocks/code bytes/metadata и явный путь при исчерпании. Не допустить неограниченного роста при уникальных RIP.
3. Считать hit/miss/compile/publication и bytes. До S3-25 освобождать только заведомо неактивные entries; активные сохранять до безопасной точки.

**Готово:** повтор программы использует block без новой compile/publication, capacity корректен, изменённый dependency не попадает в execution.

### S3-19. Подключить checked memory bridge

1. Использовать существующий GuestMemory/AccessContext и подтверждённые fault outcomes этапа 2 через узкий generated helper ABI.
2. Различать fetch/read/write, address overflow/backend error и architectural fault; не добавлять альтернативную MMU в JIT.
3. На первом пути все memory operations checked. Direct host-memory fast path рассматривать только отдельно после проверки ownership, mappings и замеров.

**Готово:** helper success/fault/internal error сохраняют правильный state и exit reason, не обходят текущие ограничения CPU contract и ISSUE-012.

### S3-20. Добавить memory lowering

1. Начать с проверенных S2 MOV load/store forms, затем подключать остальные формы по готовности interpreter/native.
2. Правильно вычислять guest effective address, widths, partial-register writes и порядок accesses. Не повторять decoder semantics вручную в emitter.
3. Сохранить commit ordering: fault не оставляет запрещённую запись в register/flags/RIP, successful store уведомляет memory owner о side effect.

**Готово:** memory fixtures с boundary/unaligned/alias cases совпадают локально с interpreter; до S3-21 это не физическая приёмка памяти.

### S3-21. Подтвердить memory slice на физическом x86

1. Использовать завершённый S2-09 native memory profile. Проверить одинаковые входы, наблюдение всех заявленных GPR, нормализацию guest addresses и сравнение memory effects.
2. Добавить ручные и generated load/store cases, пересечения regions/pages, ошибки доступов и неполное наблюдение fault metadata.
3. Сравнить interpreter/JIT/native, повторить прогоны и сохранить полные результаты. Проверять helper и allocator clobbers вместе с памятью.

**Готово:** memory subset имеет независимое подтверждение. Если S2-09 не готов, задача остаётся заблокированной; synthetic snapshots, S0 oracle и локальное совпадение не заменяют её.

### S3-22. Учесть смену execution context

1. На изменениях режима, mapping identity/permissions, необходимых segment/control state и profile выполнить invalidate/relookup либо гарантированный exit.
2. Для отсутствующих production MMU/system features проверить contract через контролируемый fixture backend и обозначить интеграционную зависимость этапа 4.
3. Не считать неизменный RIP или одинаковые bytes доказательством пригодности старого compiled block.
4. Для reset и будущего restore/clone определить очистку или перепроверку cache. Архитектурные snapshots не содержат host code pointers; executable blocks создаются заново. Реализация полного VM snapshot относится к отдельному этапу.

**Готово:** context-sensitive cases не используют несовместимый перевод. Фактическое paging/TLB поведение не заявляется на основании fixture test.

### S3-23. Инвалидировать зависимости изменённого кода

1. Ввести единый путь уведомления committed memory writes и смены backing/mapping. Подключить interpreter, JIT и управляющие writes доступного среза.
2. Изменить generation/identity нужных code regions, удалить зависимые entries из lookup и сделать невозможным stale entry по старой ссылке.
3. Проверить writable executable region, aliases, пересечение нескольких страниц, запись из другого block и повторное отображение нового backing по тому же адресу.
4. Проверить изменение code version между capture/decode и publication; устаревший результат компиляции не публиковать.

**Готово:** повторное исполнение наблюдает новые bytes. Уведомление future DMA/device writers описано как обязательная integration point, а не фиктивно реализованное устройство.

### S3-24. Обработать self-modifying code текущего блока

1. Создать case, в котором store изменяет ещё не исполненную guest instruction текущего compiled block, и case с изменением следующего блока.
2. После committed write, затрагивающего активные code dependencies, выйти с правильным guest state/RIP и продолжить через проверку актуального кода.
3. Проверить, что store выполнен ровно один раз, instruction count не удваивается, stale host tail не исполняется и переход через alias тоже учитывается.

**Готово:** результат совпадает с определённой для теста x86 семантикой и interpreter; native fixture соблюдает необходимые архитектурные правила синхронизации изменённого кода. Undefined self-modifying sequence не используется как oracle. Само удаление cache entry без остановки активного stale tail тест не проходит.

### S3-25. Реализовать eviction и безопасное освобождение

1. Развести недоступность для новых входов, active execution, references и физическое освобождение code/metadata.
2. Для одного vCPU использовать простой проверяемый retirement path на безопасных точках; не внедрять SMP reclamation framework.
3. Проверить capacity pressure, invalidation активного блока, выход по helper fault/STOP, удаление cache и повторное создание mapping.
4. После освобождения никакая entry/PC-map/link reference не должна вести в повторно используемую память прежнего блока.

**Готово:** стресс с малыми capacity не вызывает use-after-free, утечки retired blocks или исполнения освобождённого кода. Доказательства относятся к одному vCPU; SMP остаётся будущей работой.

### S3-26. Сохранить порядок side effects

1. Разметить IR memory/helper operations по реальным effects: обычная память, потенциальный fault, atomic/ordering operation, device callback при его появлении.
2. Запретить перемещение/удаление accesses через fault/commit boundaries без доказанной эквивалентности. Не считать два одинаковых адреса разрешением объединить две MMIO reads.
3. Через контролируемый backend проверить порядок и число callbacks, отсутствие повторной записи после fallback и корректные exits для неизвестных memory classes.
4. Реальные MMIO/atomic модели подключать только вместе с готовой семантикой этапов 2/4. До этого явно ограничить исполняемый профиль fixture RAM.

**Готово:** IR и lowering сохраняют observable order, faults и однократность side effects. Test callback доказывает контракт, а не поддержку виртуального устройства.

### S3-27. Проверить точное состояние при faults

1. Для каждого способа выхода сопоставить host PC/state map с faulting guest instruction и последней завершённой инструкцией.
2. Проверить GPR, flags, RIP, память и budget при faulting load/store/helper и при переходе к faulting fetch следующего блока.
3. Добавить negative tests на ошибочный early commit и потерянные live/spilled values. Unexpected host fault завершает изолированный process и даёт диагностику, а не выдуманный guest vector.
4. Если планируется direct-memory fast path, сначала отдельно доказать fault classification/recovery и ownership. До этого сохранять checked helper path.

**Готово:** все предусмотренные guest faults имеют тот же определённый snapshot, что interpreter и доступный physical oracle; runtime failures однозначно отделены.

### S3-28. Реализовать STOP и event safepoints

1. Проверять cooperative STOP/events на документированных instruction/block/helper boundaries, сохраняя ограниченный work interval.
2. Для длинного блока гарантировать конечную задержку проверки; для REP/сложного fallback использовать только готовый restart/partial-progress contract этапа 2.
3. Согласовать total budget между blocks и interpreter. Проверить STOP до entry, внутри loop, рядом с helper/fault и при исчерпании budget.
4. Снять latency на representative busy path; не обещать заданные миллисекунды без измерения. Hardware timeout не считать точным гостевым STOP.

**Готово:** остановка возвращает корректное state и не теряет/повторяет side effects; ограничение задержки описано и проверено. Установка APIC/timers этим не заявляется.

## 8. Измеряемые улучшения

Для каждой задачи этого раздела сначала сохранить корректный baseline и сравнить минимальные варианты в одинаковых условиях. Записывать compile time, warm execution, code size, spills/helper calls и ресурсы, если они меняются. Выбор «по теории должно быть быстрее» не закрывает задачу.

### S3-29. Вычислять flags по потребности

1. На ограниченном integer subset представить pending flags с width, operation и достаточными исходными values. Не терять значения после перезаписи host registers.
2. Материализовать нужные bits при Jcc/ADC/SBB и все наблюдаемые определённые bits перед exit/helper/fallback/snapshot. Учитывать операции, которые изменяют только часть flags.
3. Проверить цепочки arithmetic → partial flag writer → consumer, spills, branch exits, fault и STOP. Сравнить с eager reference path и физическим x86.
4. Измерить eager и demand-driven варианты, включая стоимость snapshots/helpers. Выбрать default по данным; eager режим может сохраняться как диагностический эталон.

**Готово:** работающая demand-driven стратегия восстанавливает точное состояние; выбранная конфигурация имеет замеры. Если отложена сама реализация flags по потребности, соответствующее обязательное требование roadmap остаётся открытым, даже при успешном eager baseline.

### S3-30. Выбрать register allocator

1. На измеренных горячих workloads определить цену spills/reloads и helper clobbers текущего простого allocator.
2. Сравнить минимальные варианты: текущий baseline и простое улучшение, например локальное удержание values или linear scan, если именно это оправдано профилем.
3. Проверить correctness при максимальном давлении на registers, разных paths, pending flags и helper calls. Сравнить compile time и code size вместе с warm throughput.
4. Если усложнение не даёт устойчивой пользы, сохранить простой вариант и записать результат эксперимента.

**Готово:** выбран простой allocator, удовлетворяющий измеренным требованиям; решение воспроизводимо. Наличие сложного allocator не является критерием успеха само по себе.

### S3-31. Оценить прямые связи блоков

1. Сначала измерить долю dispatcher/lookup в representative multi-block workload. Переходы через dispatcher уже должны корректно работать.
2. Если direct linking обоснован, реализовать один ограниченный способ связи с проверенными lifetime и invalidation dependencies. Не patch-ить active code произвольно.
3. Проверить удаление target, invalidation source/target, context change, code-memory reuse и STOP/budget на linked path.
4. Сравнить latency, compile/link cost, code size и цену invalidation. При отсутствии пользы оставить dispatcher-only и документировать решение.

**Готово:** выбранный способ переходов проверен и измерен. Direct linking необязателен; корректные межблочные переходы обязательны.

### S3-32. Оценить локальные IR-преобразования

1. Выбрать только подтверждённые профилем возможности: local constant propagation, устранение избыточного state load/store или dead pure temporary.
2. Для каждой трансформации определить допустимые types/widths и запрет на изменение faults, guest instruction accounting, flags definedness и side-effect order.
3. Выполнить A/B correctness и timing с включённой/выключенной трансформацией, включая compile cost.
4. Не добавлять speculative loads, глобальный SSA или aggressive motion через guest fault points без отдельной необходимости и доказательства.

**Готово:** внедрены только полезные и корректные преобразования либо сохранён baseline с результатами сравнения. «Меньше IR operations» не заменяет измеренную пользу.

### S3-33. Расширять lowering по готовности interpreter

1. По таблице S3-00 выбирать следующую готовую integer/branch/memory форму. Отдельно подтверждать widths, flags, faults и helper effects.
2. Для сложных поддержанных форм использовать собственный helper/interpreter fallback с точной materialization, если native lowering ещё не обоснован.
3. REP, atomics, system instructions, x87/SIMD и иные расширения не переводить приблизительно. Перед включением дождаться соответствующей semantics и oracle evidence этапа 2.
4. Для каждого profile публиковать counts native/helper/fallback/unsupported и не менять CPUID из-за одного наличия lowering.

**Готово:** выбранный JIT subset шире только за счёт реально проверенных форм; весь interpreter ISA не является обязательным native lowering этапа 3.

## 9. Проверка, производительность и интеграция

### S3-34. Собрать corpus и проверить сам harness

**Файлы:** существующие testkit, CPU harness/reducer, новые малые stage3 fixtures.

1. Объединить ручные boundary cases, фиксированные generated seeds, multi-block/fallback/memory/SMC/context/eviction cases. Не ограничивать проверку четырьмя S0 snippets.
2. Добавить reducer для JIT mismatch с сохранением profile, memory layout, code writes, branch targets и класса ошибки. Сохранять original/reduced inputs и все backend results.
3. Контролируемо проверить, что стенд ловит неправильный GPR/flag/RIP/memory byte, stale block, неверную generation, пропущенный fault и несоответствие digest/fingerprint.
4. Разделить physical-equivalence cases и implementation lifecycle tests. Тест eviction не требует физического аналога cache, но guest-visible результат обязан сохраняться.
5. Для принятого native subset запускать ручной набор и минимум 1000 уникальных допустимых generated cases по нескольким зафиксированным seeds, каждый минимум дважды. Специальные SMC/fault/process cases вести отдельным bounded набором с явным покрытием.
6. Недоступный обязательный oracle, timeout, host crash и unsupported обязательной формы дают ненулевой итог. Невыбранные suites маркируются `not_run`.

**Готово:** одна документированная команда воспроизводит correctness принятого среза; mismatch сохраняется и сокращается. 1000 cases — минимальный объём этого плана, а не доказательство всей ISA.

### S3-35. Измерить холодное исполнение

1. Измерить полный первый запуск при пустом code cache: необходимые decode/IR/lowering/allocation/publication/lookup/execution, с явно указанной стоимостью initialization.
2. Отдельно снять составляющие pipeline, не складывая несовместимые independently calibrated samples как точный end-to-end результат.
3. Для каждого cold-cache sample гарантировать отсутствие готового block; повторное использование process/runtime ресурсов явно обозначить. Cold process/VMHost launch измерять отдельно, если он входит в scenario.
4. Сравнить с interpreter и прежним fragment path там, где inputs/semantics сопоставимы. Сохранить расходы на compile и overhead fallback.

**Готово:** cold результат не замаскирован warm reuse. Замедление первого запуска допустимо как измеренный факт и должно быть отражено; обещание ускорения первого исполнения без данных запрещено.

### S3-36. Измерить прогретое исполнение

1. Прогреть cache вне timed samples и подтвердить hit/compile counters. Повторять одинаковую гостевую работу с корректным reset initial state.
2. Измерять полный steady-state runtime path с lookup/dispatcher/helpers/flags и exits, а не только одну ARM64 arithmetic instruction.
3. Чередовать варианты, применять повторные calibrated samples, сохранять raw elapsed/work counts, median и разброс; при пересекающемся шуме не утверждать speedup.
4. Отдельно показывать branch-heavy, helper-heavy и memory workloads, долю fallback, code size и resources. При осмысленности вычислить break-even числа повторов с учётом cold cost.

**Готово:** ускорение или его отсутствие установлено для конкретных workloads. Если подтверждённого ускорения целевого прогретого сценария нет, correctness slice может быть принят отдельно, а performance-критерий этапа остаётся незакрытым.

### S3-37. Проверить cache churn и ресурсы

1. Выполнить длинную серию unique blocks, повторных invalidations, capacity evictions, SMC и mixed hit/miss workloads.
2. Снимать executable bytes, metadata bytes, live/retired blocks, compile counts и доступные process memory показатели с описанием метода.
3. Проверить освобождение после STOP/fault/destroy и несколько независимых runtime instances. Не смешивать peaks разных процессов с памятью одной VM.
4. Измерить стоимость invalidation/recompile и отсутствие неограниченного накопления code/metadata. Единичный короткий smoke не выдавать за leak-free stress.

**Готово:** capacity/lifetime сохраняются при длительной нагрузке, ресурсные ограничения и рост/разброс объяснены. Полный budget работающей гостевой ОС не заявляется.

### S3-38. Интегрировать runtime с VMHost

1. Подключить новый путь через существующий host ownership/lifecycle, сохранив старые публичные ABI guarantees. Новая версия ABI нужна только при реальной несовместимости и обновляется вместе со всеми consumers.
2. Проверить RUN/RESULT/STOP, повторные вызовы, destroy после завершения и корректное владение cache между requests.
3. Обеспечить независимость двух экземпляров: guest context, code cache и resources не смешиваются. Не использовать глобальный cache без требуемого isolation contract.
4. Повторить существующие control lifecycle/fault/process regressions и новый busy-JIT STOP. Host crash остаётся аварией helper, не гостевым исключением.

**Готово:** реальный VMHost исполняет принятый JIT slice и корректно завершает/очищает его. Это не означает готовность GUI или release distribution.

### S3-39. Повторить ARM64 smoke на M1 Pro

1. Зафиксировать реально доступные OS/toolchain, способ сборки/передачи собственного artifact и выбранную ad-hoc/Hardened Runtime конфигурацию.
2. Выполнить bounded arithmetic/multi-block/helper/STOP/publication-lifecycle smoke вне debugger. Проверить отсутствие необъявленных M3-specific host instruction assumptions.
3. Сохранить конфигурацию и результат. Если M1 недоступен, указать непроверенную конфигурацию и не объявлять широкий host support.

**Готово:** повторён заявленный host smoke. M1 Pro не заменяет x86 oracle и не закрывает Developer ID/notarization вопрос ISSUE-004.

### S3-40. Зафиксировать решения по измерениям

1. Для allocator, flags, lookup/cache, linking и внедрённых IR improvements записать сравниваемые варианты, inputs, условия, samples, correctness и результат.
2. Сопоставить cold/warm/resource tradeoffs; выбрать самый простой вариант, удовлетворяющий измеренным требованиям.
3. При недостаточных данных назвать решение предварительным. Не переносить успешные данные одного short fixture на весь JIT или гостевую ОС.
4. Архитектурно значимые решения оформить ADR в существующем каталоге, остальные — в отчёте и BENCHMARKS. Сохранить нужные малые raw data и команды повторения.

**Готово:** окончательные решения критичных участков основаны на данных. Усложнения без подтверждённой пользы удалены либо не внедрены; ограничения производительности видимы.

### S3-41. Оформить приёмку этапа

1. Создать STAGE_3_REPORT: exact profiles/forms, native/helper/fallback границы, revision/fingerprint, машины/toolchains, correctness counts, commands/seeds, cold/warm/resource результаты.
2. Обновить этот план, JIT_CONTRACT, JIT.md, BENCHMARKS и README затронутых компонентов по фактическому состоянию. CPU contract менять только при действительно изменившемся архитектурном поведении.
3. Зарегистрировать нерешённые дефекты в KNOWN_ISSUES: компонент, последствия, воспроизведение, статус и следующий шаг. Решённые закрыть regression evidence. Планируемые возможности не выдавать за обнаруженные баги.
4. Сверить матрицу ниже. Незакрытый prerequisite этапа 2/4 или hardware criterion не превращать в выполненную задачу изменением формулировки.
5. Если готов только срез A/B, назвать именно его. Первый JIT не обязан нативно переводить всю ISA, но обязательные runtime/correctness/performance условия выбранного этапного профиля должны быть выполнены.

**Готово:** одинаковые программы дают одинаковое определённое состояние interpreter/JIT/physical x86; cold и warm исполнение измерены отдельно, подтверждена польза на оговорённых целевых сценариях и честно описаны регрессии/ограничения.

## 10. Матрица приёмки

| Область | Обязательное доказательство | Что не засчитывается |
|---|---|---|
| IR/lowering/emitter | Реальный pipeline и native instructions, widths/effects/verifier/relocations проверены | IR dump без исполнения, весь workload в interpreter |
| ABI/state | Layout, helper clobbers, spill state и публичные regressions | Успешный trivial call без state проверки |
| Blocks/dispatcher | Multi-block/branch/loop/budget/STOP и fallback с прогрессом | Один immutable fragment |
| Cache | Реальные hits, context/dependency validation, capacity | Lookup только по RIP без контекста |
| Code changes | Aliases, cross-page dependencies, публикация после изменения, активный SMC | Только удаление entry из lookup |
| Lifetime | Eviction/invalidation не освобождают active code/metadata | Smoke без pressure и retirement cases |
| Faults/state maps | Точный guest RIP/state на предусмотренных exits, internal errors отдельно | Любой host signal как guest exception |
| Flags | Demand-driven path с точной materialization и измеренным default | Потеря flags при helper/STOP или только NZCV |
| Memory | Нужный interpreter/native prerequisite, тройное сравнение и effects | Synthetic snapshots или register-only oracle |
| Correctness harness | Handwritten boundaries, generated corpus, repeat/replay/reducer, negative self-checks | Совпадение только собственных helpers |
| Производительность | Отдельные cold/warm samples, comparable work, разброс, подтверждённый полезный сценарий | Один лучший sample или warm число с названием cold |
| Ресурсы | Capacity/churn/retirement evidence | Неограниченный рост cache |
| Интеграция | VMHost lifecycle и заявленные host configurations | Unit tests вместо process execution |
| Документация | Фактические contracts/report/issues и повторяемые команды | Финальный ответ в чате вместо evidence |

Direct linking и сложный allocator не являются самостоятельными условиями приёмки: допустим измеренный простой вариант. Demand-driven flags и прочие явно обязательные функции roadmap остаются условиями; их перенос означает частичный результат, если требования не были отдельно изменены владельцем.

Stage 3 не доказывает полную ISA, guest boot, MMU/APIC, SMP ordering/reclamation, AMD compatibility, GPU или release signing. Поддержанные профили и границы должны следовать из отчёта, а не из названия JIT.

## 11. Проверки и команды

Существующие команды до начала реализации нового runtime:

```sh
# Локальная проверка текущего принятого стенда.
python3 tests/cpu/stage1_harness.py --suite local

# Обязательный physical oracle для текущего S0-профиля.
python3 tests/cpu/stage1_harness.py --suite oracle --generated-count 1000 --seed 0x5eedcafe --repeats 2

# Текущие fixture entry points; поддержка ограничена их фактическим профилем.
cargo run --offline -p od-runner -- --fixture CASE.json --engine interpreter --json
cargo run --offline -p od-runner -- --fixture CASE.json --engine jit --json
```

`CASE.json` заменить реальным входом. Текущий oracle/fragment CLI сам по себе не проверяет будущий block cache, memory JIT или расширенный v2 профиль.

После Rust-изменений:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --offline
cargo test -p od-jit --offline
```

Дополнительно запускать tests затронутых od-isa/od-host/od-testkit/runners, явные CPU/VMHost scenarios и physical subset соответствующей задачи. Если затронут executable code/ABI, нужны реальные ARM64 execution и process regressions. `--offline` требует доступных dependencies; отсутствие cache не считать успешной проверкой.

Для benchmarks собирать выбранные сравниваемые варианты с одинаковыми release parameters. Не включать SSH, JSON serialization и fixture generation в CPU timing, если они не являются предметом конкретного end-to-end scenario. Существующий fragment `--benchmark` расширять с явным именованием новых cold/warm режимов, а не менять смысл исторического вывода молча.

Новые stage3 CLI flags, suites и benchmark commands документировать одновременно с работающей реализацией в JIT.md. До этого не приводить их как существующий успешный путь. Автоматическая проверка должна отличать native lowering, helper и fallback, а также обязательный skip от успешного результата.

При изменении структуры/путей и для документационных задач:

```sh
python3 tools/check_layout.py
git diff --check
```

После изменения кода выполнять `graphify update .`. Временные логи/корпуса хранить в `work/`; в Git — малые собственные fixtures, необходимые raw measurements и отчёты без секретов. Не запускать CPU-тесты ради изменения только этого плана.

## 12. Первое задание для следующей модели

> Выполни только S3-01 из `docs/plans/STAGE_3.md`. Используй [JIT contract](../specs/JIT_CONTRACT.md) и проверь, что нужные fixtures/harness позволяют измерить один и тот же корректный register-only workload для существующего interpreter, текущего fragment JIT и нового block JIT. Зафиксируй воспроизводимые входы, холодное и прогретое исполнение, измеряемые границы и ограничения; не заявляй ускорение без результатов. Сохрани параллельные изменения этапа 2. Перед кодом/изменением harness зафиксируй baseline исходной реализации и сравниваемые варианты; обнови методику и отчет только по фактически полученным данным.

## 13. Статус реализации этапа

S3-00 выполнена как документационная задача. Зафиксированы точные формы S0-09; текущий прямой JIT отделён от будущего block pipeline; пути interpreter fallback помечены как будущие; этап 2 и его текущие незакоммиченные изменения не объявлены завершёнными. Записаны начальные block bounds и ARM64/ABI/macOS источники.

S3-02 выполнена: [JIT_CONTRACT](../specs/JIT_CONTRACT.md#5-runtime-boundary-s3-02) задаёт canonical state pointer/offset boundary, progress/budget outputs, stop-address и STOP/budget/fetch порядок, block exit tags, разделение compile/runtime/host ошибок, scalar helper ABI, AAPCS64 clobbers и executable mapping ownership. `od-jit::runtime` содержит `repr(C)` context, raw tags и same-build offsets; arithmetic helper semantics и legacy fragment path не менялись. GuestFault/EventPending tags зарезервированы для будущей версии. Block compiler, dispatcher и новый host invocation ещё не реализованы.

S3-03 выполнена: добавлен [линейный typed IR](../../crates/od-jit/src/ir.rs) и verifier до будущей публикации кода. IR различает temporaries и guest GPR, сохраняет guest RIP/next RIP и commit markers, описывает integer/flags effects и требует явный terminal с enum condition/kind. Verifier проверяет temp definitions/widths, GPR/flag references, последовательность commits и targets: branch target разрешается только на boundary из полного непрерывного списка guest records, branch fallthrough и exit target должны совпадать с последним `next_rip`. Эта проверка не подтверждает источник guest records; декодерная валидация принадлежит S3-04/S3-05. Целевые unit tests и cargo checks прошли; независимый diff review не выявил actionable замечаний. IR не подключён к legacy fragment compiler и не реализует lowering/runtime dispatch.

S3-04 выполнена: добавила [bounded block builder](../../crates/od-jit/src/block.rs). Он использует `long_decode::decode` по одной инструкции и read-only borrowed `CodeView`, проверяет адресные вычисления и допускает entry на любой позиции buffer/ровно code_end. Стандартные limits ограничены 32 инструкциями и 256 байтами; S0-09 forms принимаются по decoder metadata и exact encoded length. `BasicBlock` содержит decoded records и точные byte ranges, включая диапазон через границу страницы. Terminator завершает построение до декодирования хвоста. Unsupported form, limits, buffer exhaustion и deferred `DecodeBoundary` различаются; первый fetch/decode failure возвращает `NoBlock` с отложенной записью, не guest `#PF`. Mid-instruction branch target останавливает построение перед branch и сохраняет предыдущий prefix; branch target вычисляется 64-битным wrapping сложением, как в interpreter. `next_rip` текущей Jcc считается известной sequential boundary без декодирования следующих bytes; другие unparsed targets хранятся как `Unresolved`. S3-05 не передаёт unresolved target verifier как доказанную ветвь: он исключает Jcc и завершает prefix `NeedInterpreter`; dispatcher resolution остаётся S3-15. Guest ranges не содержат code identity/version: tracking относится к S3-17, cache invalidation — к S3-23. Целевые проверки прошли, независимый review не нашёл замечаний.

S3-05 выполнена: `lowering::lower` переиспользует decoded metadata для точных MOV/ADD/SUB/CMP/JZ/JNZ effects и проверяет каждый полученный Program. Arithmetic inputs фиксируются до write; CMP не пишет GPR, MOV не пишет flags. Resolved branches завершают линейные effects terminal `Branch`; unresolved Jcc исключается, а prefix передаётся с `NeedInterpreter`, без-prefix представлен `NoNativePrefix`. `DecodeBoundary` сохраняется отдельно. `CodeEnd` прокинут из `BasicBlock` и принят verifier только как точный code_end; commit+branch selection определены как одна retirement boundary. Проверки прошли: `cargo fmt --all -- --check`, `cargo check -p od-jit --all-targets --offline`, `cargo test -p od-jit --offline` (35/35), `git diff --check`; независимое review чистое.

S3-06 выполнена: существующий fragment translator использует выделенный [`arm64::Encoder`](../../crates/od-jit/src/arm64/encoder.rs) с typed GPR/SP/ZR operands, checked immediate/offset ranges, encoder-owned local labels/fixups и transactional `finish`. Encoder покрывает текущие использованные MOVZ/MOVK, arithmetic, memory, pair, conditional/unconditional branch, call и return forms; LE serialization проверяется отдельно. Writeback pair base overlap и duplicate LDP destinations отвергаются; full legacy MOV fragment имеет exact word-vector regression. [Local assembly fixture](../../crates/od-jit/tests/fixtures/arm64_encoder.s) собран Apple clang 21.0.0 для `arm64-apple-macosx15.0` и сверяется в unit test. Проверки: `cargo fmt --all -- --check`, `cargo check -p od-jit --all-targets --offline`, `cargo test -p od-jit --offline` (42/42), `git diff --check`; независимое ревью закрытых исправлений — clean, assembler fixture совпадает с 27 словами. Lowering/emission pipeline, publication, ABI и allocator не входят в этот срез.

S3-01 начата: в [BENCHMARKS](../testing/BENCHMARKS.md) описаны workload matrix и раздельные уровни cold process, cold code cache и warm execution. Preflight точной ревизии `9a1376b` описан в [отчёте baseline](../reports/STAGE_3_BASELINE_2026-10-26.md): локальные Cargo tests показали пять x87 failures; удалённый `od-isa --lib` preflight завершился ошибкой без сохранённого списка test names. Физические fixtures и timing не запускались. Из-за параллельной работы этапа 2 этот результат не переносится на последующие revisions. Повторить Stage 1 local/oracle после стабилизации соответствующих изменений; performance baseline остаётся открытым.

S3-00 прошла `python3 tools/check_layout.py`, проверку относительных Markdown links и `git diff --check`. Эти проверки подтверждают документацию/структуру, не реализацию JIT. В S3-00 код JIT, CPU/SSH-прогоны и benchmarks не изменялись и не запускались.
