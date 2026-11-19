# Этап 3. Первый JIT x86 → ARM64 — пошаговый план реализации

**Дата:** 27 октября 2026 года. **Статус:** этап начат; S3-00, S3-02, S3-03, S3-04, S3-05, S3-06, S3-07, S3-08, S3-09, S3-10, S3-11, S3-12, S3-13 и S3-14 завершены (S3-14 register-only physical gate); S3-01 в работе; S3-19b/S3-20a/S3-20b/S3-20c/S3-20d/S3-20e/S3-20f/S3-20g/S3-20h/S3-20i/S3-20j/S3-20k/S3-20l/S3-20m/S3-20n/S3-20o/S3-20p/S3-20q/S3-20r/S3-20s/S3-20t/S3-20u/S3-20v/S3-20w/S3-20x/S3-20y/S3-20z/S3-20aa/S3-20ab/S3-20ac/S3-20ad/S3-20ae/S3-20af/S3-20ag/S3-20ah/S3-20ai/S3-20aj/S3-20ak/S3-20al/S3-20am/S3-20an/S3-20ao/S3-20ap/S3-20aq/S3-20ar/S3-20as/S3-20at/S3-20au/S3-20av/S3-20aw/S3-20ax/S3-20ay/S3-20az/S3-20ba/S3-20bb/S3-20bc/S3-20bd/S3-20be/S3-20bf/S3-20bg/S3-20bh/S3-20bi/S3-20bj/S3-20bk/S3-20bl/S3-20bm/S3-20bn/S3-20bo/S3-20bp/S3-20bq/S3-20br/S3-20bs/S3-20bt/S3-20bu/S3-20bv/S3-20bw/S3-20bx/S3-20ca/S3-20cb/S3-20cc/S3-20cd/S3-20ce/S3-20cf/S3-20cg/S3-20ch/S3-20ci/S3-20cj/S3-20ck/S3-20cl/S3-20cm/S3-20cp/S3-26a/S3-26b/S3-26c/S3-26d/S3-26e/S3-27a/S3-27b/S3-22a/S3-23a/S3-23c/S3-23d/S3-23e/S3-23f/S3-23g/S3-24a/S3-24b/S3-25a/S3-25b/S3-27c/S3-34f/S3-34h/S3-34i/S3-34j реализованы как bounded partial slices; S3-20fh in progress; S3-20cm реализован как bounded optional slice, не gate этапа; S3-20cn добавил exact `8B 03` dword checked-load slice, S3-20cp добавил exact alias `8B 00`, а S3-20cq добавил exact alias store `89 00` рядом с S3-20co `89 03`. S3-20cr добавил exact three-byte form `8B 43 ib` с signed disp8, а S3-20cs — отдельную exact six-byte form `8B 83 id` с decoder-owned signed disp32 и границей `A+6`, а S3-20ct — exact three-byte dword store `89 43 ib` с signed disp8 и границей `A+3`, а S3-20cu — exact six-byte dword store `89 83 id` с signed disp32 и границей `A+6`, а S3-20cv добавил exact RIP-relative load `8B 05 id` с decoded NextRip `A+6`, а S3-20cw — exact RIP-relative store `89 05 id` с тем же decoded NextRip, а S3-20cx добавил exact addr32 load `67 8B 03` с zero-extended EBX перед canonical check, а S3-20cy — exact store `67 89 03` с тем же адресом и low32 RAX payload, а S3-20cz — exact addr32 dword load `67 8B 43 ib` с decoder-owned signed disp8, wrapped effective address и PC-map boundary `A+4`, а S3-20da добавил exact addr32 dword load `67 8B 83 id` с decoder-owned signed disp32, а S3-20db — exact addr32 dword store `67 89 43 ib` с wrapped effective address, low32 RAX payload, contiguous checked four-byte write и PC-map boundary `A+4`; остальные dword address forms остаются отдельным scope. S3-34aj pins the two S3-20cy store identities, and S3-34ak pins only the S3-20cz source/site JIT identity and wrapped-EBX host identity, with no PC-map gate; S3-34 remains open. Проверены старый полный RAX, строгая provenance, helper fault/STOP guards и PC-map phases; детали см. в task table/report. S3-27d ограничен partial slice для прямых singleton V2 load/store, S3-27e добавил partial тесты для двух-site V2, S3-27f проверяет typed-IR qword PC maps ровно для `48 8B 03` / `48 89 03`, а S3-27g добавил test-only dword PC-map contracts для `8B 03` / `89 03`; S3-27h проверил публикацию typed-IR dword load maps для disp8/disp32, их соответствие artifact map, сохранение после drop и republication; S3-27i добавил отдельные canonical-end/range-overflow checks для exact `89 83 id` через общий decoded-address path, а S3-27j проверил published typed-IR dword-store PC maps для disp8/disp32, artifact parity, retention после drop и republication. S3-34ah pins the three S3-20cw store identities, and S3-34ai pins the three S3-20cx load identities; S3-34 remains open. Direct-варианты и dword не входят в scope S3-27f. S3-27 остаётся открытой. S3-18d1 собрал три локальных dispatch/cache measurement runs без policy conclusion; S3-41a зафиксировала bounded acceptance evidence. Полный этап открыт. Общие memory lowering и S3-19/S3-20/S3-22/S3-23/S3-24/S3-25/S3-26/S3-27 остаются открытыми. S3-21 — финальный физический gate после завершения локальной работы Stage 3.

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
- Задачи `S3-50…S3-57` вынесены в отдельный [план доработки ядра этапа 3](STAGE_3_CORE.md): свести к одному механизму допуск и сборку блока, размноженные по формам (16 admission-структур, 20 вариантов `BlockBuildProfile`, 29 методов `admits_*`, 31 функция регистрового пути в `dispatch.rs`), вывести этот путь на существующий `MultiEntryBlockCache` и на interpreter fallback, измерить cold/warm в сопоставлении с записанными числами `S3-35a`/`S3-36b`. Эта линия не добавляет новых инструкций и кодировок, не ослабляет условий допуска, оставляет memory-путь `S3-19`/`S3-20` отдельным механизмом, не меняет `crates/od-isa` и `crates/od-machine` и не принимает этап 3; статусы `S3-15`/`S3-16`/`S3-17`/`S3-18` здесь обновляются её задачей `S3-57`.

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
| [x] | S3-07 | Executable publication и ownership | S3-02, S3-06 |
| [x] | S3-08 | Generated entry и helper ABI | S3-07 |
| [x] | S3-09 | Простой allocator baseline | S3-03, S3-08 |
| [x] | S3-10 | Lowering первого integer subset | S3-05, S3-06, S3-09 |
| [x] | S3-11 | Корректные eager flags | S3-10 |
| [x] | S3-12 | State points и PC maps | S3-11 |
| [x] | S3-13 | Первый сквозной запуск одного блока | S3-07, S3-12 |
| [x] | S3-14 | Тройное сравнение register-only | S3-13 |
| [partial] | S3-15 | Dispatcher, budget и переходы | S3-14 |
| [ ] | S3-16 | Interpreter fallback с прогрессом | S3-15; нужный S2 step |
| [partial] | S3-16o | Fixed-seed native→NOT RCX fallback→native Store→Store corpus and exact gate | Adds eight bounded full-state/RAM/write-ledger cases across six budget and two stop-RIP boundaries plus callback STOP; no ISA expansion. Exact Rust gate and fail-closed self-check passed locally on ARM64; Mini remains deferred and overall S3-16 remains open. См. [implementation report](../reports/STAGE_3_S3-16o_2026-11-03.md) |
| [ ] | S3-17 | Cache key и code dependencies | S3-04, S3-15 |
| [ ] | S3-18 | Ограниченный block cache | S3-07, S3-17 |
| [partial] | S3-18e | Capacity-one retained cache for exact Store→Load and Load→Store fixture pairs | See implementation report; production owner/MMU and broader S3-18 remain open |
| [partial] | S3-18f | Opt-in retained cache for the exact Load→Load pair `48 8B 03 48 8B 0B` | Sol/high review passed; full od-jit suite (397/397), `cargo check -p od-jit -p od-host --all-targets --offline`, and the exact host regression passed after ISSUE-048/032 repairs. The regression covers second-read backend error, first-load commit at `A+3`, cache eviction, one-step continuation, and republish with fresh CPU/RAM; the S3-34dr fail-closed self-test passed. Final Graphify update timed out after 120s. Stage 3 remains partial pending Mini/stage-wide checks. Store→Store remains rejected. См. [implementation report](../reports/STAGE_3_S3-18f_2026-11-03.md) |
| [partial] | S3-18g | Retained Load→Load cache invalidation on caller dependency-generation change | Exact ARM64 `od-host` acceptance checks stale resident replacement, fresh-backing execution and subsequent hit against interpreter full-state/callback parity. Exact harness gate and fail-closed self-check added. No owner/MMU changes or performance claim; Mini and stage-wide checks remain pending. См. [implementation report](../reports/STAGE_3_S3-18g_2026-11-03.md) |
| [partial] | S3-19 | Checked memory/helper bridge | S3-08, S3-16; S2-04/08 |
| [partial] | S3-19a | Invocation-local checked helper ABI и native probe | Узкий standalone slice внутри S3-19; production block exits остаются открыты |
| [partial] | S3-19b | Production V2 block/helper path для одного MOV r64 load | Точный `48 8B /r`, base/dst RAX..RBX; без cache/general IR |
| [partial] | S3-20 | Memory lowering и partial registers; проверенные MOV qword load/store slices и три точных bounded two-site pair | S3-19; interpreter соответствующих форм |
| [partial] | S3-20b | Production V2 MOV qword store с private-RAM unsafe entry | Точный `48 89 /r`, source/base RAX..RBX; остальные stores и executable aliases запрещены |
| [partial] | S3-20c | Один-byte SIB extension для V2 MOV qword load/store | ModRM `mod=00,r/m=100`; base/index/data RAX..RBX, scale 1/2/4/8, no displacement |
| [partial] | S3-20d | Signed disp8/disp32 для direct и SIB V2 MOV load/store | ModRM `mod=00/01/10`; source lengths 3/4/7 direct и 4/5/8 SIB; прочие address forms отвергаются |
| [partial] | S3-20e | RIP-relative disp32 для V2 MOV load/store | Только `mod=00,r/m=101`, ровно 7 bytes, qword и data register RAX..RBX |
| [partial] | S3-20f | Address-size override `67 48 8B/89` для V2 MOV | Только ModRM `mod=00`, qword data + dword address, RAX..RBX base/data, ровно 4 bytes |
| [partial] | S3-20g | Signed disp8/disp32 для V2 MOV с address-size override | `67 48 8B/89 /r`, ModRM `mod=01/10`, qword data + dword address, base/data RAX..RBX, exact lengths 5/8; без SIB и других форм |
| [partial] | S3-20h / S3-26a | Checked qword load через общий BasicBlock → typed IR → allocator → runtime emitter | Только `48 8B /r`, ModRM `mod=00`, data/base RAX..RBX, без SIB/displacement; V1/direct V2 остаются отдельными |
| [partial] | S3-20i / S3-26b | Checked qword store через общий BasicBlock → typed IR → allocator → runtime emitter | Только `48 89 /r`, ModRM `mod=00`, source/base RAX..RBX; отдельный typed entry, no replay после helper write |
| [partial] | S3-20j / S3-26c / S3-20k / S3-26d | Один generated entry для ровно двух exact pairs: `48 89 03; 48 8B 0B` и `48 8B 03; 48 89 02` | Только private synchronous ordinary RAM; two guest commits; STOP/fault/budget continuation bounded; singleton APIs сохранены |
| [partial] | S3-26e | Exact native load→load pair `48 8B 03; 48 8B 0B`; две отдельные checked reads `[RBX]` в RAX и RCX | Callback trace, full interpreter parity, second-site partial fault/retry, STOP/budget/event guards; private ordinary RAM only |
| [partial] | S3-26f | Exact uncached store→store pair `48 89 03; 48 89 02`; two ordered checked writes `[RBX]`, `[RDX]` from RAX | Exact first-site builder admission repaired; four focused od-jit gates, full od-jit suite (397/397), and exact host lifecycle gate pass. S3-34dq corpus/harness gates also pass; Stage 3/Mini/physical-x86 and production owner/MMU acceptance remain open; private ordinary RAM only. См. [report](../reports/STAGE_3_S3-26f_2026-11-03.md) |
| [partial] | S3-20l | RIP-relative qword MOV через shared IR и bounded memory dispatcher | Только singleton `48 8B/89 /r`, ModRM `mod=00,r/m=101`, disp32, data RAX..RBX; exact 7-byte source; no wider block/prefix/SIB |
| [partial] | S3-20m | Direct-base signed disp8/disp32 MOV через shared IR и bounded dispatcher | Только singleton `48 8B/89 /r`, ModRM `mod=01/10`, base/data RAX..RBX, exact length 4/7; no SIB/prefix/wider block |
| [partial] | S3-20n | No-displacement SIB MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, ровно 4 bytes |
| [partial] | S3-20o | Address32 direct-base qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r`, ModRM `mod=00`, Dword EA/Qword data, base/data RAX..RBX, ровно 4 bytes |
| [partial] | S3-20p | Address32 signed-displacement qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, Dword EA/Qword data, base/data RAX..RBX, ровно 5/8 bytes |
| [partial] | S3-20q | 64-bit SIB signed-displacement qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, base/index/data RAX..RBX, scale1/2/4/8, ровно 5/8 bytes |
| [partial] | S3-20r | Address32 no-displacement SIB qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, data/base/index RAX..RBX, scale1/2/4/8, ровно 5 bytes |
| [partial] | S3-20s | Address32 signed disp8 SIB qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, data/base/index RAX..RBX, scale1/2/4/8, ровно 6 bytes |
| [partial] | S3-20t | Address32 signed disp32 SIB qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, data/base/index RAX..RBX, scale1/2/4/8, ровно 9 bytes; singleton source metadata bound 9 bytes |
| [partial] | S3-20u | RBP-base signed disp8 qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r disp8`, ModRM `mod=01,r/m=101`, RBP base/data RAX..RBX, ровно 4 bytes; decoder preserves RBP #SS semantics |
| [partial] | S3-20v | RSP-base qword MOV через exact no-index SIB `0x24` в shared IR и bounded dispatcher | Только `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, SIB `0x24`, RSP base, no index/displacement, data RAX..RBX, ровно 4 bytes |
| [partial] | S3-20w | RSP-base qword MOV через exact no-index SIB `0x24` + disp8 | Только `48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, RSP base/no index, ровно 5 bytes |
| [partial] | S3-20x | RSP-base qword MOV через exact no-index SIB `0x24` + disp32 | Только `48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, RSP base/no index, ровно 8 bytes |
| [partial] | S3-20y | RBP-base signed disp32 qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r disp32`, ModRM `mod=10,r/m=101`, RBP base/data RAX..RBX, ровно 7 bytes; RIP-relative `mod=00/rm=101` остаётся отдельной формой |
| [partial] | S3-20z | RSP как data register в qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r`, ModRM `mod=00`, data RSP, base RAX..RBX, ровно 3 bytes; generic GPR admission не расширяется |
| [partial] | S3-20aa | RBP как data register в qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r`, ModRM `mod=00`, data RBP, base RAX..RBX, ровно 3 bytes; generic GPR admission не расширяется |
| [partial] | S3-20ab | RBP одновременно data и base для qword MOV с signed disp8 | Только `48 8B/89 6D ib`, ModRM `mod=01/reg=RBP/rm=RBP`, ровно 4 bytes; адрес использует старый RBP |
| [partial] | S3-20ac | RSP одновременно data и base в qword MOV через exact SIB | Только `48 8B/89 24 24`, ModRM `mod=00/rm=100`, SIB `0x24`, ровно 4 bytes; адрес использует старый RSP |
| [partial] | S3-20ad | RSP одновременно data и base в qword MOV через exact SIB + signed disp8 | Только `48 8B/89 64 24 ib`, ModRM `mod=01/reg=RSP/rm=100`, SIB `0x24`, ровно 5 bytes; адрес и store payload используют старый RSP |
| [partial] | S3-20ae | RSP одновременно data и base в qword MOV через exact SIB + signed disp32 | Только `48 8B/89 A4 24 id`, ModRM `mod=10/reg=RSP/rm=100`, SIB `0x24`, ровно 8 bytes; адрес и store payload используют старый RSP, байты displacement 4..7 сохраняются |
| [partial] | S3-20af | RBP одновременно data и base в qword MOV через signed disp32 | Только `48 8B/89 AD id`, ModRM `mod=10/reg=RBP/rm=RBP`, ровно 7 bytes; адрес и store payload используют старый RBP |
| [partial] | S3-20ag | RBP как data и RSP как base через exact no-index SIB | Только `48 8B/89 2C 24`, ModRM `mod=00/reg=RBP/rm=SIB`, SIB `0x24`, ровно 4 bytes |
| [partial] | S3-20ai | RSP как data и RBP как base через direct disp8/disp32 | Только `48 8B/89 65 ib` или `48 8B/89 A5 id`, ModRM `mod=01/10,reg=RSP,rm=RBP`, ровно 4/7 bytes; `mod=00/rm=5` остаётся RIP-relative |
| [partial] | S3-20aj | RSP/RBP как data через direct disp8/disp32 от RAX..RBX | Только `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, `reg=RSP|RBP`, `rm=RAX..RBX`, ровно 4/7 bytes; без SIB |
| [partial] | S3-20ak | RIP-relative qword MOV с RSP/RBP data через shared IR и bounded dispatch | Только `48 8B/89 25/2D disp32`, ModRM `mod=00,r/m=101`, `reg=RSP|RBP`, ровно 7 bytes; EA от NextRip текущей инструкции |
| [partial] | S3-20al | RSP/RBP data через indexed SIB qword MOV | Только существующие 64-bit indexed SIB формы с ModRM `reg=RSP|RBP`, base/index RAX..RBX, scale 1/2/4/8; lengths 4/5/8 |
| [partial] | S3-20am | RSP/RBP data через address32 direct-base qword MOV | Только `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, `reg=RSP|RBP`, base RAX..RBX; длины 4/5/8 |
| [partial] | S3-20an | RSP/RBP data через address32 indexed-SIB qword MOV | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, data RSP/RBP, base/index RAX..RBX, scale 1/2/4/8; длины 5/6/9 |
| [partial] | S3-20ao | RSP/RBP data через address32 no-index SIB qword MOV | Только `67 48 8B/89 /r 24 [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB `0x24`, старый RSP как низко-32-битная база, data RAX..RBP; длины 5/6/9 |
| [partial] | S3-20ap | RSP/RBP data через address32 direct EBP-base qword MOV | Только `67 48 8B/89 /r disp8|disp32`, ModRM `mod=01/10,r/m=101`, старый EBP/RBP low32 base, data RAX..RBP; длины 5/8, `mod=00/rm=101` исключён |
| [partial] | S3-20aq | RSP/RBP data через indexed SIB с RSP base | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длины 4/5/8 |
| [partial] | S3-20ar | RSP/RBP data через indexed SIB с RBP base | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=RBP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длины 5/8; `mod=00/base=101` остаётся no-base и запрещён |
| [partial] | S3-20as | Address32 qword MOV через indexed SIB с RSP base | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20at | Address32 qword MOV через indexed SIB с RBP base | Только `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=RBP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 6/9; `mod=00/base=101` остаётся no-base и запрещён |
| [partial] | S3-20au | Absolute qword MOV через no-base/no-index SIB disp32 | Только `48 8B/89 /r 25 disp32`, ModRM `mod=00,r/m=100`, SIB ровно `0x25`, data RAX..RBP; длина 8; адрес — sign-extended disp32 |
| [partial] | S3-20av | Address32 absolute qword MOV через no-base/no-index SIB disp32 | Только `67 48 8B/89 /r 25 disp32`, ModRM `mod=00,r/m=100`, SIB ровно `0x25`, data RAX..RBP; Dword EA/Qword data; длина 9 |
| [partial] | S3-20aw | Address64 indexed no-base SIB disp32 qword MOV | Только `48 8B/89 /r sib disp32`, ModRM `mod=00,r/m=100`, SIB base=101/index RAX..RBX/scale 1/2/4/8, data RAX..RBP; длина 8; SIB `0x25` no-index остается отдельным profile |
| [partial] | S3-20ax | Address32 indexed no-base SIB disp32 qword MOV | Только `67 48 8B/89 /r sib disp32`, ModRM `mod=00,r/m=100`, SIB base=101/index RAX..RBX/scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer; длина 9 |
| [partial] | S3-20ay | Address64 no-displacement SIB qword MOV с индексом RBP | Только `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP; длина 4; остальные индексные профили остаются exact |
| [partial] | S3-20ba | Address32 indexed SIB qword MOV with RBP index | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20bb | Address64 indexed SIB qword MOV with RSP base and RBP index | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP/index=RBP, scale 1/2/4/8, data RAX..RBP; длины 4/5/8 |
| [partial] | S3-20bc | Address64 indexed SIB qword MOV with RBP base and RBP index | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP; длины 5/8; `mod=00/base=101` остаётся no-base |
| [partial] | S3-20bd | Address32 indexed SIB qword MOV with RSP base and RBP index | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP/index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20be | Address32 indexed SIB qword MOV with RBP base and RBP index | Только `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 6/9; `mod=00/base=101` остаётся no-base |
| [partial] | S3-20bg | Address32 indexed no-base SIB qword MOV with RBP index | Только `67 48 8B/89 /r sib disp32`, ModRM `mod=00/rm=100`, SIB base=101/index=RBP/scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длина 9; 20bf Qword profile остаётся отдельным |
| [partial] | S3-20bh | Address64 direct-base qword MOV через RSI/RDI | Только `48 8B/89 /r`, ModRM `mod=00/rm=110|111`, data RAX..RBP; RSI/RDI — base only, no SIB/displacement/prefix; длина 3 |
| [partial] | S3-20bi | Address64 direct-base qword MOV через RSI/RDI с displacement | Только `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10/rm=110|111`, data RAX..RBP; RSI/RDI — base only; длины 4/7 |
| [partial] | S3-20bj | Address32 direct-base qword MOV через RSI/RDI | Только `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, `rm=110|111`, data RAX..RBP; длины 4/5/8; EA по low32 базы и signed displacement |
| [partial] | S3-20bk | Address64 indexed SIB qword MOV с RSI/RDI base | Только `48 8B/89 /r sib`, ModRM `mod=00/rm=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длина 4 |
| [partial] | S3-20bl | Address64 indexed SIB qword MOV с RSI/RDI base и displacement | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длины 5/8 |
| [partial] | S3-20bm | Address32 indexed SIB qword MOV с RSI/RDI base | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20bn | Address64 indexed SIB qword MOV с RSI/RDI base и RBP index | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index RBP, scale 1/2/4/8, data RAX..RBP; Qword EA/data; длины 4/5/8 |
| [partial] | S3-20bp | Address64 no-displacement SIB qword MOV with RSI/RDI index | Только `48 8B/89 /r sib`, ModRM `mod=00/rm=100`, base RAX..RBX, index RSI/RDI, scale 1/2/4/8, data RAX..RBP; длина 4 |
| [partial] | S3-20br | Address32 indexed SIB qword MOV с low base и RSI/RDI index | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, base RAX..RBX, index RSI/RDI, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20bw | Address64 indexed no-base SIB qword MOV с RSI/RDI index и disp32 | Только `48 8B/89 /r sib disp32`, ModRM `mod=00/rm=100`, SIB base=101 (no base), index RSI/RDI, scale 1/2/4/8, data RAX..RBP; длина 8; адрес — full-width index×scale + signed disp32 по модулю u64 |
| [partial] | S3-20bx | Address32 indexed no-base SIB qword MOV с RSI/RDI index и disp32 | Только `67 48 8B/89 /r sib disp32`, ModRM `mod=00/rm=100`, SIB base=101 (no base), index RSI/RDI, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer; длина 9; адрес — `(low32(index)×scale + signed disp32) mod 2^32` |
| [partial] | S3-20ca | Address64 low-base SIB qword MOV без индекса | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB `0x20..=0x23`, base RAX..RBX, data RAX..RBP; scale 1/no index; lengths 4/5/8 |
| [partial] | S3-20cb | Address32 low-base SIB qword MOV без индекса | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB `0x20..=0x23`, base RAX..RBX, data RAX..RBP; scale 1/no index; lengths 5/6/9 |
| [partial] | S3-20cc | Address64 RBP-base SIB qword MOV без индекса с displacement | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB ровно `0x25`, data RAX..RBP, длины 5/8; mod00 SIB `0x25` остаётся отдельной absolute/no-base формой |
| [partial] | S3-20cd | Address32 RBP-base no-index SIB qword MOV с displacement | Только `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB ровно `0x25`, data RAX..RBP; Dword EA/Qword data; длины 6/9; mod00 остаётся отдельной absolute/no-base формой |
| [partial] | S3-20ce | Address64 RSI/RDI-base SIB qword MOV без индекса | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB `0x26/0x27`, data RAX..RBP; scale 1/no index; lengths 4/5/8 |
| [partial] | S3-20cf | Address32 RSI/RDI-base SIB qword MOV без индекса | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB ровно `0x26/0x27`, data RAX..RBP; Dword EA/Qword transfer; lengths 5/6/9 |
| [partial] | S3-20ch | Address64 direct-base qword load into RSI/RDI | Только `48 8B /r`, ModRM `mod=00`, destination RSI/RDI, base RAX..RBX; без SIB/displacement/prefix; длина 3; scoped IR write и 8-GPR runtime frame |
| [partial] | S3-20ci | Address64 direct low-base qword MOV with RSI/RDI data and signed displacement | Только `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, data RSI/RDI, base RAX..RBX; длины 4/7, scoped load/store IR permissions |
| [partial] | S3-20cj | Address32 direct low-base qword MOV with RSI/RDI data and signed displacement | Только `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, data RSI/RDI, base RAX..RBX; Dword EA/Qword transfer; lengths 4/5/8 |
| [partial] | S3-20cn | Exact unprefixed dword checked-memory load `8B 03` (`MOV EAX,[RBX]`) through shared IR | Только одна инструкция/форма; four-byte read with EAX zero-extension; wider memory lowering and additional address forms remain open |
| [partial] | S3-20cp | Exact aliased dword checked-memory load `8B 00` (`MOV EAX,[RAX]`) alongside S3-20cn | Только эта alias форма рядом с `8B 03`; callback использует старый полный RAX, затем EAX zero-extension; проверены no-commit faults/guards, `A+2` PC map and strict source/descriptor admission. Owner/MMU, другие формы и dispatcher остаются вне scope. См. [report](../reports/STAGE_3_S3-20cp_2026-11-01.md) |
| [partial] | S3-20co | Exact unprefixed dword checked-memory store `89 03` (`MOV [RBX],EAX`) through shared typed IR | Только singleton и одна address/data форма; broader dword stores and memory lowering remain open. См. [report](../reports/STAGE_3_S3-20co_2026-11-01.md) |
| [partial] | S3-20cq | Exact aliased dword checked-memory store `89 00` (`MOV [RAX],EAX`) alongside S3-20co | Только alias `[RAX]`/`EAX`; old full RAX supplies address and low32 payload; exact 4-byte callback, strict provenance, fault/STOP/budget and PC-map checks. Owner/MMU, other forms and dispatcher remain out of scope. См. [report](../reports/STAGE_3_S3-20cq_2026-11-01.md) |
| [partial] | S3-20cr | Exact dword checked-memory load `8B 43 ib` (`MOV EAX,[RBX+disp8]`) through shared typed IR | Только 3-byte singleton; decoder-owned signed disp8 from full RBX, 256 source identities, one four-byte read/zero-extension, `A+3` boundary; other dword forms remain closed. См. [report](../reports/STAGE_3_S3-20cr_2026-11-01.md) |
| [partial] | S3-20cs | Exact dword checked-memory load `8B 83 id` (`MOV EAX,[RBX+disp32]`) through shared typed IR | Только 6-byte singleton; decoder-owned signed disp32 from full RBX, exact source/site identity and `A+6` PC-map boundary, one four-byte read with EAX zero-extension. Other dword forms remain closed; S3-20 and Stage 3 stay open. См. [report](../reports/STAGE_3_S3-20cs_2026-11-01.md) |
| [partial] | S3-20ct | Exact dword checked-memory store `89 43 ib` (`MOV [RBX+disp8],EAX`) through shared typed IR | Только 3-byte singleton; decoder-owned signed disp8 from full RBX, exact four-byte write and `A+3` boundary. The existing S3-20co/S3-20cq store identities remain separate; S3-20 stays open. См. [report](../reports/STAGE_3_S3-20ct_2026-11-01.md) |
| [partial] | S3-20cu | Exact dword checked-memory store `89 83 id` (`MOV [RBX+disp32],EAX`) through shared typed IR | Только 6-byte singleton; decoder-owned signed disp32 from full RBX, exact source/site identity and `A+6` PC-map boundary, one checked four-byte write. Separate S3-27i regression now covers canonical-end/range-overflow via the shared decoded-address path; no reported runtime defect. См. [report](../reports/STAGE_3_S3-20cu_2026-11-01.md) |
| [partial] | S3-20cv | Exact RIP-relative dword checked-memory load `8B 05 id` (`MOV EAX,[RIP+disp32]`) through shared typed IR | Только 6-byte singleton; decoder-owned NextRip is `A+6`, exact source/site binding and six-byte PC-map boundary, guest address derives from NextRip. Other dword addressing forms stay separate. См. [report](../reports/STAGE_3_S3-20cv_2026-11-01.md) |
| [partial] | S3-20cw | Exact RIP-relative dword checked-memory store `89 05 id` (`MOV [RIP+disp32],EAX`) through shared typed IR | Только 6-byte singleton; decoded NextRip is `A+6`, source bytes bind to the site and the PC map has the six-byte boundary. Host fixture has split code/data backing; `(ENTRY,-1)` writes only BTreeMap data RAM, so it is not SMC or backing-alias evidence. См. [report](../reports/STAGE_3_S3-20cw_2026-11-01.md) |
| [partial] | S3-20cx | Exact address-size-32 dword checked-memory load `67 8B 03` (`MOV EAX,[EBX]`) through shared typed IR | Только 3-byte singleton; decoder-owned address calculation zero-extends low32 RBX before canonical checking, exact source/site identity, `A+3` PC-map boundary and checked four-byte load/EAX zero-extension. Other addr32 forms remain closed. См. [report](../reports/STAGE_3_S3-20cx_2026-11-02.md) |
| [partial] | S3-20cy | Exact address-size-32 dword checked-memory store `67 89 03` (`MOV [EBX],EAX`) through shared typed IR | Только 3-byte singleton; zero-extended low32 RBX address before canonical checking, low32 RAX payload and one checked four-byte write. Other addr32 forms remain closed. См. [report](../reports/STAGE_3_S3-20cy_2026-11-02.md) |
| [partial] | S3-20cz | Exact address-size-32 disp8 dword checked-memory load `67 8B 43 ib` (`MOV EAX,[EBX+disp8]`) through shared typed IR | Только 4-byte singleton; signed disp8 and address-size-32 wrapping/zero-extension follow decoded operand, one checked four-byte load, source/site binding, and `A+4` PC-map boundary. Other addr32 forms remain closed. См. [report](../reports/STAGE_3_S3-20cz_2026-11-02.md) |
| [partial] | S3-20da | Exact address-size-32 disp32 dword checked-memory load `67 8B 83 id` (`MOV EAX,[EBX+disp32]`) through shared typed IR | Только 7-byte singleton; decoder-owned signed disp32 and addr32 wrap/zero-extension; one checked four-byte load, source/site identity and `A+7` boundary. Other addr32 forms remain closed. См. [report](../reports/STAGE_3_S3-20da_2026-11-02.md) |
| [partial] | S3-20dl | Exact address-size-32 no-displacement SIB dword loads `67 8B 04 0B/4B/8B/CB` (`MOV EAX,[EBX+ECX*1/2/4/8]`) through shared typed IR | Ровно четыре 4-byte singletons; decoder-owned addr32 EA wraps low32 EBX + low32 ECX×scale, then zero-extends before canonical checking; one contiguous checked width-4 read including a read crossing the 4-GiB boundary; EAX write zero-extends RAX; prefix/scale/site binding, `A+4` PC map, STOP/budget/fault. Author-worktree full `od-jit` 324/324; full `od-host` 150/153 because three ignored manual-corpus artifacts were unavailable. См. [report](../reports/STAGE_3_S3-20dl_2026-11-02.md) |
| [partial] | S3-20dm | Exact address-size-32 no-displacement SIB dword stores `67 89 04 0B/4B/8B/CB` (`MOV [EBX+ECX*1/2/4/8],EAX`) through shared typed IR | Ровно четыре 4-byte singletons; decoder-owned addr32 sum wraps low32 EBX + low32 ECX×scale then zero-extends before canonical checking; high halves of RBX/RCX are ignored; one contiguous checked width-4 write of low32 RAX; `A+4`, source binding, PC map, errors/STOP/budget. Author-branch `od-jit` 325/325; full `od-host` was not run. См. [report](../reports/STAGE_3_S3-20dm_2026-11-02.md) |
| [partial] | S3-20dn | Exact address-size-32 SIB dword checked loads with signed disp8 `67 8B 44 0B/4B/8B/CB ib` (`MOV EAX,[EBX+ECX*1/2/4/8+disp8]`) through shared typed IR | Ровно четыре 5-byte singletons × all 256 signed disp8 values; decoder-owned addr32 EA wraps low32 EBX + low32 ECX×scale + disp8 then zero-extends; one checked contiguous width-4 read, EAX zero-extends into RAX, `A+5`; all 4×256 source/site identities, PC map, atomic faults, STOP/budget. Author-worktree `od-jit` 326/326 before cherry-pick; full `od-host` not run. См. [report](../reports/STAGE_3_S3-20dn_2026-11-02.md) |
| [partial] | S3-20do | Exact address-size-32 SIB dword checked stores with signed disp8 `67 89 44 0B/4B/8B/CB ib` (`MOV [EBX+ECX*1/2/4/8+disp8],EAX`) through shared typed IR | Ровно четыре 5-byte singletons × all 256 signed disp8 values; decoder-owned addr32 EA wraps low32 EBX + low32 ECX×scale + disp8 then zero-extends before canonical checking; one contiguous checked width-4 write of low32 RAX; `A+5`, all 4×256 source/site identities, PC map, atomic faults, STOP/budget. Author-worktree `od-jit` 327/327; `od-host` 153/156 because three ignored manual-corpus/cache artifacts were unavailable. См. [report](../reports/STAGE_3_S3-20do_2026-11-02.md) |
| [partial] | S3-20dp | Exact address-size-32 SIB dword checked loads with signed disp32 `67 8B 84 0B/4B/8B/CB id` (`MOV EAX,[EBX+ECX*1/2/4/8+disp32]`) through shared typed IR | Ровно четыре 8-byte singletons; decoder-owned addr32 sum wraps low32 EBX + low32 ECX×scale + signed disp32, then zero-extends before canonical checking; high RBX/RCX halves ignored; one contiguous checked width-4 read, EAX zero-extends into RAX, `A+8`. Source/site binding for 4 scales × all disp32 bytes, PC map, faults, STOP/budget. Author-worktree before cherry-pick: `od-jit` 328/328, `od-host` 154/157 (three ignored manual artifacts unavailable). См. [report](../reports/STAGE_3_S3-20dp_2026-11-02.md) |
| [partial] | S3-20dq | Exact address-size-32 SIB dword checked stores with signed disp32 `67 89 84 0B/4B/8B/CB id` (`MOV [EBX+ECX*1/2/4/8+disp32],EAX`) through shared typed IR | Ровно четыре 8-byte singletons; decoder-owned addr32 sum wraps low32 EBX + low32 ECX×scale + signed disp32, then zero-extends before canonical checking; high RBX/RCX halves ignored; one contiguous checked width-4 write of low32 RAX, `A+8`. Source/site binding distinguishes every disp32 byte and scale; PC-map/fault/STOP/budget cases. Author-worktree: `od-jit` 329/329; `od-host s3_20 filter` 28/28; focused exact identities passed. Full `od-host` suite not run. Host fixture is private synchronous RAM only; no MMIO, SMC, owner notification, or MMU integration is claimed. Mini/M1/Stage 2 deferred. См. [report](../reports/STAGE_3_S3-20dq_2026-11-02.md) |
| [partial] | S3-20dr | Exact RBP-base dword checked-memory load with signed disp8 `8B 45 ib` (`MOV EAX,[RBP+disp8]`) through shared typed IR | Только 3-byte singleton; all 256 signed disp8 source/site identities; decoder-owned full-width RBP + signed disp8 with wrapping; one checked four-byte read, EAX zero-extension, `A+3`. Host parity covers `MAX+1 → 0`, `AddressOverflow` at `MAX-1` for width 4 without callback/state commit, and RBP noncanonical start/end `#SS`; retained RBX-based S3-20cr host gate supplies the `#GP` control. State/budget/STOP behavior is covered. `od-jit` 330/330; exact JIT/host gates, preserved S3-20cr host identity and RBP dispatcher regression passed; targeted package checks, fmt/diff/Graphify passed. Full `od-host`, hardware, Mini/M1/Stage 2 and owner/MMU integration were not tested. См. [report](../reports/STAGE_3_S3-20dr_2026-11-02.md) |
| [partial] | S3-20ds | Exact RBP-base dword checked-memory store with signed disp8 `89 45 ib` (`MOV [RBP+disp8],EAX`) through shared typed IR | Только 3-byte singleton; all 256 signed disp8 source/site identities; decoder-owned full-width RBP + signed disp8 with wrapping; one checked four-byte write of low32 RAX. PC-map marks `HelperCall` and `AfterHelper` `InadmissibleAfterEffect`, `CommitInProgress` `InadmissibleDuringCommit`, and the post-commit boundary `A+3`. Host checks RBP noncanonical start/end `#SS` versus RBX `#GP`, `MAX+1 → 0`, width-4 `AddressOverflow` without write/state commit, fault atomicity, budget/STOP, and no replay after committed STOP. `od-jit` 331/331; exact new gates and preserved S3-20ct host gate passed 1/1; targeted package checks, fmt/diff/Graphify passed. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20ds_2026-11-02.md) |
| [partial] | S3-20dt | Exact RBP-base dword checked-memory load with signed disp32 `8B 85 id` (`MOV EAX,[RBP+disp32]`) through shared typed IR | Только 6-byte singleton; twelve signed disp32 descriptors bind all four displacement bytes to distinct sites, not an exhaustive 2^32-value sweep; decoder-owned full-width RBP plus signed disp32 with wrapping; one checked width-4 read, EAX zero-extension, and the `A+6` guest boundary. Host checks RBP noncanonical-start/end `#SS` against the retained RBX disp32 `#GP` control, `MAX+1 → 0`, width-4 range overflow before callback/commit, fault atomicity and STOP/budget. `od-jit` 332/332; exact new gates and preserved S3-20dr/S3-20cs JIT+host regressions passed; targeted `od-jit`/`od-host` check, fmt/diff/Graphify passed. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20dt_2026-11-02.md) |
| [partial] | S3-20du | Exact RBP-base dword checked-memory store with signed disp32 `89 85 id` (`MOV [RBP+disp32],EAX`) through shared typed IR | Только 6-byte singleton; JIT tests use 12 representative disp32 values plus source-byte mutations (not an exhaustive 2^32 sweep); host parity success loop covers 10 values. Decoder-owned full-width RBP plus signed disp32 with wrapping; one checked width-4 write of low32 RAX; `A+6`. Host checks `#SS` start/end, wrap/overflow, fault atomicity and STOP/budget/no replay. In-test RBX `#GP` control is `89 43 01` disp8; separate S3-20cu tests cover RBX disp32. `od-jit` 333/333; exact new gates plus S3-20ds and S3-20cu regressions passed; targeted checks, fmt/diff/Graphify passed. No separate PC-map claim. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20du_2026-11-02.md) |
| [partial] | S3-20dv | Exact address-size-32 RBP-base dword checked-memory load with signed disp8 `67 8B 45 ib` (`MOV EAX,[EBP+disp8]`) through shared typed IR | Только 4-byte singleton; JIT binds all 256 disp8 bytes to distinct source/site identities and checks `A+4`; effective address wraps low32 EBP + signed disp8 modulo 2^32 then zero-extends. Host covers the two signed edges `-128`/`+127` against low/high RBP values, with one contiguous checked width-4 read crossing 4 GiB at `0xffffffff`; result zero-extends into RAX and state/errors/STOP are checked. No addr64 RBP `#SS` claim. `od-jit` 334/334; both exact gates passed 1/1; retained S3-20dr JIT+host and S3-20cz host regressions passed 1/1. Affected check, fmt/diff/Graphify and Sol review passed. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20dv_2026-11-02.md) |
| [partial] | S3-20dw | Exact address-size-32 RBP-base dword checked-memory store with signed disp8 `67 89 45 ib` (`MOV [EBP+disp8],EAX`) through shared typed IR | Только 4-byte singleton; decoder-owned low32 EBP + signed disp8 wraps modulo 2^32 then zero-extends; one contiguous width-4 write, including across 4 GiB, with guest boundary `A+4`. JIT binds all 256 disp8 bytes to source/site identities. Host checks six encoded displacement/base pairs across low/high RBP values, native/interpreter state and RAM parity, budget/STOP, and one attempted callback write with no CPU/RAM commit on callback errors. `od-jit` 335/335; exact new gate and S3-20db/20dv/20ds JIT+host regressions passed 1/1. Affected check, fmt/diff/Graphify and Sol review passed. No address-size-64 RBP `#SS` or separate PC-map gate claim. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20dw_2026-11-02.md) |
| [partial] | S3-20dx | Exact address-size-32 RBP-base dword checked-memory load with signed disp32 `67 8B 85 id` (`MOV EAX,[EBP+disp32]`) through shared typed IR | Только 7-byte singleton; JIT tests eight representative signed disp32 values and mutates all four encoded displacement bytes to verify source/site binding (not an exhaustive 2^32 sweep); decoder-owned low32 EBP + signed disp32 wraps modulo 2^32 then zero-extends. One contiguous checked width-4 read, including across 4 GiB, writes EAX with zero-extension into RAX; guest boundary `A+7`. Host uses eight `(disp32, low32 EBP)` tuples, each with low/high full-width RBP, and compares native/interpreter state/RAM. `od-jit` 336/336; exact new gates and preserved S3-20dv/S3-20da JIT+host regressions passed 1/1. Affected check, fmt/diff/Graphify and Sol review passed. No address-size-64 RBP `#SS` claim. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20dx_2026-11-02.md) |
| [partial] | S3-20dy | Exact address-size-32 RBP-base dword checked-memory store with signed disp32 `67 89 85 id` (`MOV [EBP+disp32],EAX`) through shared typed IR | Только 7-byte singleton; JIT uses 11 signed disp32 representatives and four-byte source mutations (not exhaustive over 2^32); decoder-owned low32 EBP + signed disp32 wraps then zero-extends. One contiguous checked width-4 write of low32 EAX, including across 4 GiB; guest boundary `A+7`. Host covers 10 displacement/base tuples × low/high full-width RBP; callback error causes one attempted write and no CPU/RAM commit. `od-jit` 337/337; exact new gate plus S3-20dw JIT+host, legacy addr64 RBP disp32-store JIT, and S3-20dc host regressions passed 1/1. Affected check, fmt/diff/Graphify and runtime Sol review passed. No separate PC-map phase or addr64 RBP `#SS` claim. Full `od-host`, Mini, M1, Stage 2 not run. См. [report](../reports/STAGE_3_S3-20dy_2026-11-02.md) |
| [partial] | S3-20dz | Exact address64 RSP-based dword checked-memory load `8B 04 24` (`MOV EAX,[RSP]`) through shared typed IR | Только 3-byte SIB scale-1/no-index singleton; full-width RSP EA, exactly four-byte checked read, EAX zero-extends into RAX, boundary `A+3`. Host checks RSP `#SS` at noncanonical start/end vs RBX `#GP`, width-four range overflow before read callback, injected-error state atomicity, and callback STOP commit. Retrying the same block with committed RIP `A+3` returns `EntryRipMismatch { expected: A, actual: A+3 }`, preserves state, and performs no second read. New exact JIT/host gates and adjacent S3-20cr/20cs/20dd host regressions passed 1/1; `od-jit` 338/338. Affected cargo check, fmt/diff/Graphify and fresh Sol review passed. No separate PC-map phase claim. Full `od-host`, Mini, M1, Stage 2 not run. См. [report](../reports/STAGE_3_S3-20dz_2026-11-02.md) |
| [partial] | S3-20ea | Exact address64 RSP-based dword checked-memory store `89 04 24` (`MOV [RSP],EAX`) through shared typed IR | Только 3-byte SIB scale-1/no-index singleton; full-width RSP EA, one contiguous checked width-4 write of low32 RAX, boundary `A+3`. Host checks RSP `#SS` at noncanonical start/end vs RBX `#GP`, width-four `AddressOverflow` before callback, backend-error atomicity, entry STOP/budget, and callback STOP commit. Retrying same block at committed RIP `A+3` returns `EntryRipMismatch { expected: A, actual: A+3 }`, leaves state unchanged, and performs no second write. Exact JIT/host gates and S3-20dz/S3-20de host regressions passed 1/1; `od-jit` 339/339. Affected check, fmt/diff/Graphify and fresh Sol review passed. No separate PC-map phase claim. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20ea_2026-11-02.md) |
| [partial] | S3-20eb | Exact address64 RSP-base dword checked-memory load with signed disp8 `8B 44 24 ib` (`MOV EAX,[RSP+disp8]`) through shared typed IR | Только 4-byte SIB singleton; JIT binds all 256 signed disp8 source/site values. Full-width RSP plus signed disp8 wraps in u64; one contiguous checked width-4 read, EAX zero-extension, guest boundary `A+4`. Host checks `+1` and `-1` (including RSP `0x1001` → EA `0x1000`), `u64::MAX + 1 → 0`, interpreter parity, and one width-four callback read; also #SS vs RBX #GP, overflow/read-error atomicity, STOP and retry `EntryRipMismatch` at committed `A+4` without a second read. Exact new test and preserved S3-20dz JIT+host and S3-20ea host gates passed 1/1; `od-jit` 340/340. Targeted checks, fmt/diff/Graphify and Sol review passed. No PC-map phase claim. Full `od-host`, Mini, M1 and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-20eb_2026-11-02.md) |
| [partial] | S3-20ec | Exact address64 RSP-base dword checked-memory store with signed disp8 `89 44 24 ib` (`MOV [RSP+disp8],EAX`) through shared typed IR | Только four-byte SIB singleton, scale 1/no index; decoded full-width RSP plus signed disp8 uses wrapping u64 arithmetic. The checked helper writes low32 RAX in one contiguous width-4 operation and commits `A+4`; JIT binds all 256 displacement bytes to exact signed metadata and sites. Host checks ±disp8, wrap to EA 0, full CPU/RAM parity, one callback, RSP `#SS` vs RBX `#GP`, overflow/backend atomicity, entry/callback STOP and replay rejection at `A+4`. On author runtime commit `7ae5578`, `od-jit` passed 341/341 and `od-host` 170/170 with a fresh writable corpus directory seeded from immutable baseline inputs. Adjacent S3-20ea/20eb exact regressions passed 1/1 on author runtime commit `7ae5578`. After integration as `c9195cd` runtime + `2edeb07` harness, the new exact JIT/host gates passed 1/1 and Python harness 87/87. `cargo check`, fmt, Graphify and independent Sol review passed. No separate PC-map or performance claim. Stage 3 remains partial; Mini/M1 deferred, Stage 2 not run. См. [report](../reports/STAGE_3_S3-20ec_2026-11-02.md) |
| [partial] | S3-20ed | Exact address64 RSP-base dword checked-memory load with signed disp32 `8B 84 24 id` (`MOV EAX,[RSP+disp32]`) through shared typed IR | Только 7-byte SIB singleton; full-width RSP plus sign-extended disp32 uses wrapping u64 arithmetic; one contiguous checked width-4 read zero-extends EAX into RAX and commits `A+7`. JIT checks nine representative signed disp32 values and binds all four encoded bytes to distinct source/site identities; it is not an exhaustive 2^32 sweep. Host checks native/interpreter parity, one width-four read, negative and wrap-to-zero addresses, RSP `#SS` vs RBX `#GP`, overflow/read-error atomicity, STOP/budget and no-replay at `A+7`. On author revision `c9530b1`, `od-jit` passed 342/342, exact JIT/host gates and adjacent S3-20dz/S3-20eb host regressions passed 1/1; runtime Sol review, cargo check, and fmt passed. After root integration as `238799d`, exact JIT/host gates passed 1/1 and the full Python Stage 3 harness passed 88/88; Graphify update and clean-tree check passed. Full `od-host`, Mini/M1 and Stage 2 were not run. No broad PC-map or performance claim. Stage 3 remains partial. См. [report](../reports/STAGE_3_S3-20ed_2026-11-02.md) |
| [partial] | S3-20ee | Exact address64 RSP-base dword checked-memory store with signed disp32 `89 84 24 id` (`MOV [RSP+disp32],EAX`) through shared typed IR | Только 7-byte SIB singleton, scale 1/no index; full-width RSP plus signed disp32 uses wrapping u64 arithmetic. One contiguous checked width-4 write stores low32 RAX and commits `A+7`. JIT binds nine representative signed displacements and all four displacement bytes to exact source/site metadata; it is not exhaustive across 2^32 values. Host checks native/interpreter state and RAM parity, one width-four callback, wrap to EA 0, RSP `#SS` versus RBX `#GP`, overflow/backend-write-error atomicity, entry STOP/budget and callback STOP commit, and no replay at `A+7`. On author runtime commit `7a07588`, `od-jit` passed 343/343; exact S3-20ee JIT/host and adjacent S3-20ec JIT/host gates passed 1/1. After root integration (`6cac944` runtime, `1f32667` harness), exact S3-20ee gates passed 1/1 and the full Python Stage 3 harness passed 89/89. Runtime Sol review, formatting, and affected-crate check passed on the author branch; root Graphify was refreshed to 12,037 nodes. No full `od-host`, broad PC-map, performance, or hardware claim. Stage 3 remains partial; Mini/M1 deferred. См. [report](../reports/STAGE_3_S3-20ee_2026-11-02.md) |
| [partial] | S3-20ef | Exact address-size-32 ESP-base dword checked-memory load `67 8B 04 24` (`MOV EAX,[ESP]`) through shared typed IR | Только этот four-byte no-index SIB singleton. Effective address is `u64(RSP as u32)`: high 32 RSP bits are ignored, and the low32 ESP offset is zero-extended before the contiguous width-four read. A read starting at `0xffff_fffe` spans through `0x1_0000_0001`; individual bytes do not wrap at 4 GiB. EAX zero-extends into RAX; other registers and flags are preserved; boundary `A+4`. JIT binds exact source/site and rejects indexed/neighbor encodings; the existing negative S3-20dz JIT gate remains separate. Host compares interpreter state/RAM, checks poisoned high32 values, one read, the cross-4GiB span, injected read-error atomicity, STOP/budget and callback retry with no second read. The runtime review fix restored the existing indexed-base guard; the accepted form remains no-index ESP only. On author runtime commit `392886b`, new exact JIT/host gates, the existing negative JIT gate, and adjacent S3-20cx/S3-20cz host gates passed 1/1; `od-jit` passed 344/344, with `cargo check`, fmt/diff, Graphify, and Sol review passing. After root integration (`a0ce871` runtime, `1ba352a` harness), exact S3-20ef gates passed 1/1 and the full Python Stage 3 harness passed 91/91; root Graphify refresh reported 12,048 nodes. No RSP `#SS` or other unreachable address-derived fault claim, full `od-host`, broad PC-map, performance, or hardware claim. Stage 3 remains partial; Mini/M1 deferred. См. [report](../reports/STAGE_3_S3-20ef_2026-11-02.md) |
| [partial] | S3-20eg | Exact address-size-32 ESP-base dword checked-memory store `67 89 04 24` (`MOV [ESP],EAX`) through shared typed IR | Только этот four-byte no-index SIB singleton. Effective address is `u64(RSP as u32)`; high RSP bits are ignored and the low32 ESP offset is zero-extended. One contiguous checked width-four write stores low32 RAX; a start at `0xffff_fffe` spans through `0x1_0000_0001` without per-byte wrap; boundary `A+4`. Host compares native/interpreter state and RAM, poisoned-high32 bases, callback count/width, STOP/budget and callback-STOP retry without a second write. Runtime follow-up `525e310` adds successful retry assertions after backend errors and entry STOP/budget guards; independent Sol/high static review passed. The updated exact host gate passed 1/1, closing ISSUE-045 for those assertions; the S3-34br fail-closed harness self-test also passed. Historical evidence: author runtime `abd2af4` passed `od-jit` 345/345 and formatting/check/review; at root `8052314` with harness `ac069ca`, exact gates passed 1/1 and Python Stage 3 harness passed 91/91. Those historical results predate `525e310`. No full `od-host`, performance, broad PC-map or physical-hardware claim. Mini SSH to `senya@192.168.1.14:22` timed out before any remote command or test; M1 remains deferred. Stage 3 remains partial. См. [report](../reports/STAGE_3_S3-20eg_2026-11-02.md) |
| [partial] | S3-20eh | Exact address-size-32 ESP-base dword checked-memory load with signed disp8 `67 8B 44 24 ib` (`MOV EAX,[ESP+disp8]`) through shared typed IR | Только этот 5-byte no-index SIB singleton; JIT binds all 256 signed disp8 encodings to exact source/site metadata and `A+5`. Effective address wraps `(low32(RSP) + sign-extended disp8)` modulo 2^32, then zero-extends; the width-four read is contiguous and may cross 4 GiB without per-byte wrap. EAX zero-extends into RAX; host checks high32-poisoned RSP, wrap at ESP 0/u32::MAX, state/RAM parity, one read, callback STOP commit and replay rejection without a second read. On author runtime `62a516a`, `od-jit` passed 346/346; exact JIT/host gates and adjacent S3-20ef/S3-20cz host gates passed 1/1; fmt/check and Sol runtime review passed. After integration at root `08b3eba` with harness `046f5a2`, exact JIT/host gates passed 1/1 and the full Python Stage 3 harness passed 92/92 after 34bs integration. No full `od-host`, broad PC-map, performance, or hardware claim. Stage 3 remains partial; Mini/M1 deferred. См. [report](../reports/STAGE_3_S3-20eh_2026-11-02.md) |
| [partial] | S3-20ei | Exact address-size-32 ESP-base dword checked-memory store with signed disp8 `67 89 44 24 ib` (`MOV [ESP+disp8],EAX`) through shared typed IR | Только этот 5-byte no-index SIB singleton; EA is `(low32(RSP) + sign-extended disp8) mod 2^32`, zero-extended, and high RSP bits are ignored. One contiguous checked width-four write stores low32 RAX, including across 4 GiB; boundary `A+5`. JIT binds all 256 signed disp8 source/site encodings. Host checks state/RAM parity, address wrap, poisoned high32, entry STOP/budget and backend-error no-commit with successful retries, plus callback STOP commit and no second write on retry. On author runtime `cb02fb8`, `od-jit` passed 347/347; exact S3-20ei JIT/host gates and adjacent S3-20eg/S3-20eh exact gates passed 1/1, with cargo check/fmt and Sol review PASS. Runtime `cb02fb8` integrated as `2784629`; harness source `42719175` integrated as `d8e1065`. At root `2784629`, exact JIT/host gates passed 1/1 and Python Stage 3 harness passed 93/93, with 34bt tests present. No full `od-host`, separate PC-map, broad performance, or hardware claim. ISSUE-045 remains open for the distinct S3-20eg coverage limitation and is not closed by this slice. Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. См. [report](../reports/STAGE_3_S3-20ei_2026-11-02.md) |
| [partial] | S3-20ej | Exact address-size-32 ESP-base dword checked-memory load with signed disp32 `67 8B 84 24 id` (`MOV EAX,[ESP+disp32]`) through shared typed IR | Только этот eight-byte no-index SIB singleton. EA wraps `(low32(RSP) + sign-extended disp32) mod 2^32`, then zero-extends; high RSP bits are ignored. A contiguous checked width-four read can cross 4 GiB without per-byte wrap, zero-extends EAX to RAX, and commits `A+8`. JIT checks ten signed disp32 representatives and all four source bytes against exact sites. Host checks `-128`/`+127`, wrap-to-zero, poisoned high32, interpreter parity, cross-4GiB span, and entry STOP/budget successful retries; callback STOP commits once and retry reads no second time. On author runtime `d649bd4`, `od-jit` passed 348/348; cargo check/fmt, Graphify, Sol runtime review, exact S3-20ej JIT/host gates and adjacent S3-20da/S3-20dx host gates passed 1/1. Runtime `d649bd4` integrated as `4e4fec2`; harness source `00430e9` integrated as `4790e76`. At root `4e4fec2`, exact JIT/host gates passed 1/1 and Python Stage 3 harness passed 94/94; harness manifest: 216 gates (206 acceptance, 10 lifecycle), 234 local steps. No full `od-host`, broad PC-map, performance, or hardware claim. ISSUE-045 remains open for the distinct S3-20eg coverage gap; this slice does not close it. Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. См. [report](../reports/STAGE_3_S3-20ej_2026-11-02.md) |
| [partial] | S3-20ek | Exact address-size-32 ESP-base dword checked-memory store with signed disp32 `67 89 84 24 id` (`MOV [ESP+disp32],EAX`) through shared typed IR | Только этот eight-byte no-index SIB singleton. EA wraps `(low32(RSP) + sign-extended disp32) mod 2^32`, then zero-extends; high RSP bits are ignored. A contiguous checked width-four write stores low32 RAX, can cross 4 GiB without per-byte wrap, and commits `A+8`. JIT binds six representative signed disp32 values and source-byte mutations. Author runtime `34ad7e2`: exact JIT gate passed 1/1, `od-jit` 349/349, cargo check/fmt, Graphify and Sol runtime review passed. The initial exact host run occurred before the execution-deferral message reached the reviewer and failed only on a fixture expected address (`0x7fff_ffff`); it was corrected to `0x8000_1fff` and statically Sol-reviewed but not rerun while Mini execution remains deferred. Host gate is **pending**, not passing. Runtime `34ad7e2` integrated as `190a61c`; the Python Stage 3 harness passed 94/94 at root `190a61c`. Full `od-host` was not run; separate 34bv harness gates are outside this slice. Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. См. [report](../reports/STAGE_3_S3-20ek_2026-11-02.md) |
| [partial] | S3-20el | Exact absolute SIB disp32 Dword load `8B 04 25 id` (`MOV EAX,[disp32]`) through shared typed IR | Только семибайтный unprefixed singleton: no base/index, scale 1, Qword address size and Dword transfer; signed disp32 is sign-extended to the 64-bit absolute EA (`-4` → `u64::MAX-3`). One checked width-four read; EAX zero-extends to RAX and commits `A+7`; JIT binds nine signed displacement representatives and all source bytes/site identity. On author runtime `aa8e38f`, `cargo fmt`, `cargo check -p od-jit -p od-host --tests --offline`, exact JIT gate 1/1, and diff-check passed on Mac15,11. This execution violated the user's Mini deferral due to mistaken orchestration; it is not a deferred Mini/M1 gate. Host exact test only compiled and was not executed. Cleanup `d483a63` integrated as `0903953`; static Sol review and diff-check passed, with no tests/build. Full `od-jit`/`od-host` and Mini/M1 physical gates remain pending. ISSUE-045 remains open in [the issue registry](../KNOWN_ISSUES.md). Stage 2 remained outside this slice's evidence; concurrent runs were ignored per user instruction. Stage 3 remains partial. См. [report](../reports/STAGE_3_S3-20el_2026-11-02.md) |
| [partial] | S3-20em | Exact absolute SIB disp32 Dword store `89 04 25 id` (`MOV [disp32],EAX`) through shared typed IR | Только семибайтный unprefixed singleton: SIB без base/index, scale 1, Qword address size и Dword transfer; signed disp32 sign-extends в абсолютный 64-bit EA (`-4` → `u64::MAX-3`). Одна checked width-four запись low32 RAX и boundary `A+7`; JIT фиксирует девять representative signed disp32 и все байты источника/site. Host fixture подтверждает ровно одну запись на `u64::MAX-3`, отсутствие коммита при `-3/-2/-1` range faults и backend error с успешным retry, entry STOP/budget без записи, callback STOP после единственного коммита и отсутствие повторной записи. Runtime source `c6149ae34f9b74516f301ab6016f8f87d9def24a` интегрирован как `5d8e9ac`; runtime Sol/high re-review PASS. По Mini deferral для 20em не запускались тесты, сборки, fmt или Cargo checks; exact JIT и host gates pending до итоговых проверок этапа. Harness `b165126c83378cbc69168f5201dc3ddabd340d48` интегрирован как `a70a022`; статическая manifest проверка: 222 gates (212 acceptance, 10 lifecycle), 240 local steps, harness review PASS; Python тесты harness не запускались. ISSUE-045 остаётся открытым. Stage 2 вне evidence этой задачи; конкурентные запуски игнорировались по инструкции пользователя. Mini/M1 отложены; Stage 3 частичный. См. [report](../reports/STAGE_3_S3-20em_2026-11-02.md) |
| [partial] | S3-20en | Exact address-size-32 absolute SIB disp32 Dword load `67 8B 04 25 id` (`MOV EAX,[disp32]`) through shared typed IR | Только восьмибайтный singleton: addr32 no-base/no-index SIB, scale 1, Dword address/transfer; decoder-owned disp32 wraps modulo 2^32 then zero-extends. Одна checked width-four read может пересечь 4 GiB без per-byte wrap; EAX zero-extends, boundary `A+8`. JIT binds all eight source bytes and nine displacement representatives. Host fixture covers read at `0xffff_ffff`, state preservation, entry guards, backend-error no-commit/retry, callback STOP commit/no replay. Runtime `4234628` integrated as `6588d50`; static Sol review PASS; identity aligned with 34by harness in `ba6d8e1`. No tests/build/fmt/Cargo/Python checks were run due Mini deferral; exact Cargo gates remain pending. Harness `082d05d` integrated as `9353bfa`, 224 gates (214 acceptance, 10 lifecycle), 242 local steps; static review PASS. ISSUE-045 remains open. Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [report](../reports/STAGE_3_S3-20en_2026-11-02.md) |
| [partial] | S3-20eo | Exact address-size-32 absolute SIB disp32 Dword store `67 89 04 25 id` (`MOV [disp32],EAX`) through shared typed IR | Только восьмибайтный singleton: addr32 no-base/no-index SIB, scale 1, Dword address/transfer; decoder-owned disp32 wraps modulo 2^32 then zero-extends. One checked contiguous width-four write stores low32 RAX, can cross 4 GiB without per-byte wrap, and commits `A+8`. JIT binds all eight source bytes and six displacement representatives. Host fixture covers `0xffff_fffe`, backend-error no-commit/retry, entry guards, callback STOP commit/no replay. Runtime `c98a28d` integrated as `4970c5c`; static Sol/high review PASS after coverage fixes. No tests/build/fmt/Cargo/Python checks due Mini deferral; exact runtime gates pending. Harness `7ee99dc` integrated as `5069141`, 226 gates (216 acceptance, 10 lifecycle), 244 steps; static review PASS, no Python tests. ISSUE-045 remains open. Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [report](../reports/STAGE_3_S3-20eo_2026-11-02.md) |
| [partial] | S3-20ep | Exact address64 RCX-indexed no-base SIB disp32 Dword load `8B 04 0D disp32` (`MOV EAX,[RCX+disp32]`) through shared typed IR | Только unprefixed seven-byte singleton: SIB без base, RCX index scale 1, Dword transfer; decoder-owned EA uses full RCX plus signed disp32 with wrapping u64 arithmetic. One checked contiguous width-four read; EAX zero-extends, boundary `A+7`. Runtime `ee36c4c` integrated as `07c2beb`; Sol/high static review PASS after fixing IR profile nesting and host mapping lease lifetime. No tests/build/fmt/Cargo/Python checks due Mini deferral; exact runtime gates pending. Harness S3-34ca pins both exact identities; source `8b05d2f` integrated `02c92fa`, 228 gates (218 acceptance, 10 lifecycle), 246 local steps; static review PASS, no Python execution. ISSUE-045 remains open. Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [report](../reports/STAGE_3_S3-20ep_2026-11-02.md) |
| [partial] | S3-20eq | Exact address64 RCX-indexed no-base SIB disp32 Dword store `89 04 0D disp32` (`MOV [RCX+disp32],EAX`) through shared typed IR | Только unprefixed seven-byte singleton: SIB без base, RCX index scale 1, Dword transfer; decoder-owned EA uses full RCX plus signed disp32 with wrapping u64 arithmetic. One checked contiguous width-four write stores low32 RAX; boundary `A+7`. Runtime `c384bce` integrated as `8c50c90`; Sol/high static review PASS after fixes to compiler/block/host byte gates and host overflow/backend expectations. No tests/build/fmt/Cargo/Python checks due Mini deferral; exact runtime gates pending. Harness S3-34cb pins both exact identities; historical source `e16f6fd` integrated `3fa9448`, 230 gates (220 acceptance, 10 lifecycle), 248 local steps; static review PASS, no Python/harness execution. Newer aggregate S3-34cc totals are not attributed to 34cb. ISSUE-045 remains open. Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [report](../reports/STAGE_3_S3-20eq_2026-11-02.md) |
| [partial] | S3-20er | Exact address64 RCX-indexed no-base SIB disp32 Dword loads at scales 2/4/8 (`8B 04 4D/8D/CD disp32`) through shared typed IR | Только семь-byte unprefixed singletons: SIB без base, RCX index at scale 2, 4, or 8, Dword transfer; decoder-owned EA uses full-width `RCX*scale + sign-extended disp32` with wrapping u64 arithmetic. One contiguous checked width-four read; EAX zero-extends, boundary `A+7`. Runtime `0ad1f51` integrated as `edf50fe`; Sol/high static review PASS after stale 4D/8D negatives were removed and source-forge scale 2 vs original 0D was preserved. No tests/build/fmt/Cargo/Python checks due Mini deferral; exact runtime gates pending. Harness S3-34cc pins identities; source `af854bf` integrated `0567f37`, historical manifest 232 gates (222 acceptance, 10 lifecycle), 250 steps; static review PASS, no Python execution. ISSUE-045 remains open. Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [report](../reports/STAGE_3_S3-20er_2026-11-02.md) |
| [partial] | S3-20db | Exact address-size-32 disp8 dword checked-memory store `67 89 43 ib` (`MOV [EBX+disp8],EAX`) through shared typed IR | Только 4-byte singleton; decoder-owned wrapped/zero-extended addr32 EA, low32 RAX payload, one contiguous checked four-byte write, all 256 disp8 source/site identities, and `A+4` PC-map boundary with STOP/budget/fault/no-replay coverage. Other addr32 store forms remain closed. См. [report](../reports/STAGE_3_S3-20db_2026-11-02.md) |
| [partial] | S3-20dc | Exact address-size-32 disp32 dword checked-memory store `67 89 83 id` (`MOV [EBX+signed disp32],EAX`) through shared typed IR | Только 7-byte singleton; decoder-owned signed disp32 wraps in addr32 and zero-extends the EA, low32 RAX is written by one contiguous checked four-byte store, and the PC-map boundary is `A+7`. Source/site identity, STOP/budget/fault/no-replay behavior are covered; other addr32 store forms remain closed. См. [report](../reports/STAGE_3_S3-20dc_2026-11-02.md) |
| [partial] | S3-20dd | Exact address64 SIB dword checked-memory load `8B 04 0B` (`MOV EAX,[RBX+RCX]`) through shared typed IR | Только этот 3-byte scale-1/no-displacement singleton; decoder-owned address64 EA adds full-width RBX and RCX, checked width is four bytes; EAX write zero-extends into RAX and clears `RAX[63:32]`, while other registers/flags are preserved; boundary `A+3`. Source/site, PC-map, fault/no-replay, STOP, and budget remain fixture-bounded. Distinct scale and displacement forms are covered separately by S3-20df/dh/dj. См. [report](../reports/STAGE_3_S3-20dd_2026-11-02.md) |
| [partial] | S3-20df | Exact address64 SIB dword checked loads `8B 04 4B` / `8B 04 8B` / `8B 04 CB` (`MOV EAX,[RBX+RCX*2|4|8]`) through shared typed IR | Ровно эти три 3-byte singletons: decoder-owned address48 EA uses full-width RBX + full-width RCX×2/4/8; four-byte read writes EAX and clears `RAX[63:32]`; `A+3` boundary, strict source/site and PC-map, faults/guards. No scale-specific canonical-end/range-overflow fixture was added; shared preflight/helper are unchanged, with no runtime defect claimed. См. [report](../reports/STAGE_3_S3-20df_2026-11-02.md) |
| [partial] | S3-20dg | Exact address64 SIB dword checked stores `89 04 4B` / `89 04 8B` / `89 04 CB` (`MOV [RBX+RCX*2|4|8],EAX`) through shared typed IR | Ровно эти три 3-byte singletons: decoder-owned full-width EA; one contiguous checked four-byte write of low32 EAX; `A+3`; strict source/site scale binding, PC map/no-replay, fault atomicity, STOP, and budget. Boundary fixture checks decoded EA ending `...fffe` and crossing `#GP`; scale-1 S3-20de store stays separately covered. См. [report](../reports/STAGE_3_S3-20dg_2026-11-02.md) |
| [partial] | S3-20dh | Exact address64 SIB dword checked loads `8B 44 0B/4B/8B/CB ib` (`MOV EAX,[RBX+RCX*1/2/4/8+disp8]`) through shared typed IR | Ровно четыре 4-byte singletons × all 256 signed disp8 values; decoder-owned `access_address_48` uses full-width RBX/RCX and encoded scale, then signed disp8. Each four-byte read writes EAX and clears `RAX[63:32]`; `A+4`, source/site binding across all 4×256 combinations, PC map, atomic faults, STOP/budget. No separate canonical-end/range-overflow fixtures were added; shared preflight is unchanged, no defect claimed. См. [report](../reports/STAGE_3_S3-20dh_2026-11-02.md) |
| [partial] | S3-20di | Exact address64 SIB dword checked stores `89 44 0B/4B/8B/CB ib` (`MOV [RBX+RCX*1/2/4/8+disp8],EAX`) through shared typed IR | Ровно четыре 4-byte singletons × all 256 signed disp8 values; decoder-owned full-width RBX/RCX × scale plus sign-extended disp8; low32 RAX через один contiguous checked four-byte write, `A+4`, source/site binding and PC map. Host fixture distinguishes canonical-end `#GP` from u64 range overflow, with no write/commit on either fault and a successful adjacent valid write; STOP/budget are covered. See [report](../reports/STAGE_3_S3-20di_2026-11-02.md) |
| [partial] | S3-20dj | Exact address64 SIB dword checked loads `8B 84 0B/4B/8B/CB id` (`MOV EAX,[RBX+RCX*1/2/4/8+signed disp32]`) through shared typed IR | Ровно четыре 7-byte singletons; decoder-owned `access_address_48` uses full-width RBX/RCX, encoded scale and signed disp32; four-byte load writes EAX/zero-extends RAX, boundary `A+7`; all 4 disp32 source bytes bind at each scale, with PC map, atomic faults, STOP/budget. Host covers canonical-end `#GP`, u64 overflow, and valid adjacent read. См. [report](../reports/STAGE_3_S3-20dj_2026-11-02.md) |
| [partial] | S3-20dk | Exact address64 SIB dword checked stores `89 84 0B/4B/8B/CB id` (`MOV [RBX+RCX*1/2/4/8+signed disp32],EAX`) through shared typed IR | Ровно четыре 7-byte singletons; decoder-owned EA from full-width RBX/RCX, SIB scale and signed disp32; one contiguous checked width-4 write of low32 RAX; `A+7`, source/site binding, PC map, fault/STOP/budget/no-replay, canonical-end and range-overflow fixtures. Validation is author-worktree/pre-cherry-pick; no full `od-jit` suite claim. См. [report](../reports/STAGE_3_S3-20dk_2026-11-02.md) |
| [partial] | S3-20de | Exact address64 SIB dword checked-memory store `89 04 0B` (`MOV [RBX+RCX],EAX`) through shared typed IR | Только этот 3-byte scale-1/no-displacement singleton; decoder-owned EA sums full-width RBX and RCX; low32 RAX is written once as a contiguous checked four-byte access and the boundary is `A+3`. Source/site, PC map, no-replay, fault atomicity, STOP, and budget behavior remain fixture-bounded. Distinct SIB displacement-store profiles are covered by S3-20di. См. [report](../reports/STAGE_3_S3-20de_2026-11-02.md) |
| [ ] | S3-21 | Финальный физический gate для memory slice после локального Stage 3 | S3-20; S2-09 и нужный native profile |
| [partial] | S3-22 | Смена execution/mapping context | S3-18, S3-21 |
| [partial] | S3-22a | Bounded S0_09 fixture context generation/profile in translation keys and cached dispatch | Current synchronous compile/publish holds a caller-owned frozen snapshot; production owner lease and revalidation for any future async/unfrozen compilation remain open |
| [partial] | S3-23a | Fixture owner notifications для изменения backing/mapping и cache invalidation между sync dispatch calls | Только неактивные entries; current synchronous caller holds frozen snapshot; production owner/writers and future async/unfrozen candidate commit remain open |
| [partial] | S3-23b | Shared `CodeInvalidation` path for capacity-one `MemoryPairCache` | Fixture notifications between calls only; host ARM64 proves MAP_JIT release before republish; production owner/MMU and active SMC remain open |
| [partial] | S3-23c | Fixture-only V2 generated store to a different tracked executable backing | One vCPU/fixed RAM; active-source overlap rejected; no active-code retirement or production MMU |
| [partial] | S3-23d | Shared fixture write transaction for interpreter MOV, V2 store, and control writes | Interpreter and control adapters commit between calls; production MMU/DMA/MMIO/SMP remain open |
| [partial] | S3-23e | Region-scoped executable-write notification and redispatch | Separate store entry invalidates cached prefix/tail and aliases before next lookup; no active native tail or S3-24 claim |
| [partial] | S3-23f | Fixture qword store across two adjacent mappings of one backing | At most two contiguous same-backing mappings; all-or-nothing single-slice copy and one version/notification |
| [partial] | S3-23g | Fixture qword store across two adjacent mappings with distinct BackingIds | Fixed two-segment plan, one-vCPU copy transaction, one version/notification per backing; no hardware atomicity or SMP |
| [ ] | S3-23 | Инвалидация code dependencies | S3-18, S3-21, S3-22; полный writer/MMU integration |
| [ ] | S3-24 | Self-modifying code в текущем блоке | S3-23 |
| [partial] | S3-24a | Active native three-instruction artifact exits after committed store changes already executed prefix | One typed V3 slot/call; future-tail writes rejected; future-instruction SMC oracle and general blocks remain open |
| [partial] | S3-24b | Exact qword rewrite of the not-yet-executed V3 tail immediate | Exact 23-byte fixture, one same-backing segment at entry+15..entry+23; stale tail exits before execution; no physical SMC oracle or general S3-24 closure |
| [partial] | S3-24c | Exact V3 tail-immediate rewrite across adjacent guest mappings | One- or two-segment destination on contiguous offsets of one backing; one commit/version/notification, dirty exit, cached-tail invalidation and fresh dispatch; fixture owner only, no production MMU or general S3-24 closure. См. [report](../reports/STAGE_3_S3-24c_2026-11-03.md) |
| [partial] | S3-27a | Exact V3 store architectural fault payload and precise prefix state | Fixture-injected #PF after checked write entry; host decoder negatives; full S3-27 fault/fetch/PC-map coverage remains open |
| [partial] | S3-27b | Fault on interpreter fetch at `A+6` after one exact two-site pair | Native pair commits are compared against interpreter-only steps; fixture-only boundary evidence, production MMU/dispatcher remains open |
| [partial] | S3-27c | Host-PC attribution map for exact V3 active-code fixture | Validated contiguous ARM64 byte ranges and three committed boundaries; map retained through artifact drop/cache invalidation/slot reuse; diagnostic only, direct V2 and full S3-27 remain open |
| [partial] | S3-27d | Host-PC attribution for direct singleton V2 load/store | Direct wrappers own validated maps; standalone publication and arena store retain generation-bound copies; no fault recovery, memory-class expansion, Stage2, Mini, or full S3-27 closure |
| [partial] | S3-27e | PC-map attribution for two-site V2 memory entries | Compiler checks cover all checked ranges/boundaries and reject targeted map mutations; host test checks published-map identity/persistence across republication; no host-fault recovery or full S3-27 closure |
| [partial] | S3-27f | Typed-IR qword PC maps for exact `48 8B 03` / `48 89 03` load/store | Compiler comparator covers checked ranges/boundaries; host checks artifact-map equality, map retention after artifact drop, and distinct replacement-map publication; excludes direct variants and dword; no host-fault recovery or full S3-27 closure |
| [partial] | S3-27g | Typed-IR dword PC maps for exact `8B 03` / `89 03` load/store | Compiler checks one committed boundary at `A+2`, helper-effect phase classification, and malformed-map rejection; host checks published-map retention and republication; test-only, no host-fault recovery or full S3-27 closure. См. [report](../reports/STAGE_3_S3-27g_2026-11-01.md) |
| [partial] | S3-27h | Published typed-IR dword load PC maps for disp8 and disp32 | Exact map/artifact parity, retention across artifact drop, and republish coverage for the bounded load profiles; S3-34ac pins this integration-test identity. Test-only, no host-fault recovery or full S3-27 closure. См. [report](../reports/STAGE_3_S3-27h_2026-11-01.md) |
| [partial] | S3-27i | Canonical-end and range-overflow host checks for exact `89 83 id` dword store | Separate host regression exercises end-canonical and 64-bit range-overflow validation through the shared decoded `access_address_48` path; no new instruction admission, production owner/MMU, or full S3-27 fault recovery. См. [report](../reports/STAGE_3_S3-27i_2026-11-01.md) |
| [partial] | S3-27j | Published dword-store PC-map parity and lifetime for disp8/disp32 profiles | Exact artifact-map parity, retention after artifact drop, and republication for the bounded store profiles; S3-34af pins the integration-test identity. Test-only; no host-fault recovery or full S3-27 closure. См. [report](../reports/STAGE_3_S3-27j_2026-11-01.md) |
| [x] | S3-25 | Eviction, retirement и освобождение — принято с ограничениями (gate-ревью Claude Opus, high, 2026-11-18; закрытие S3-25a–d) | S3-18, S3-24; ограничения: один vCPU, синхронно, без cross-thread release/grace period (SMP-reclamation вне S3 — S6-40/S6-42); нормативная реализация — `PagedFetchedLoadSession` (одна инструкция на вызов; доказательства `s4_15f_*`/`s4_16i_*`), стресс S3-25d — только fixture-арена `MultiEntryBlockCache`+`PublishedArenaBackend`, ёмкость 1/2, macOS aarch64 (на других платформах тест пуст); текущий профиль блоков (≤32 инструкций по контракту, ≤9–10 через compile pipeline из-за `MAX_EMITTED_BLOCK_BYTES = 4096`; memory-блоки ≤2 сайтов; без связей — правило проверки поколения на переходах обязательно для S3-31); инвалидация активного блока — только точный V3 fixture (S3-24a/b) и одноинструкционный store production (S4-16i), SMC общих многоинструкционных блоков со store — S3-24 (открыт); политика вытеснения/ёмкость — S3-18 (открыт); writers DMA/NVMe/display вне, опора на проверку ключа при lookup (S4-16h); «освобождённый код не исполнялся» — косвенно (сравнение с interpreter, отказ stale handles до эффекта), без счётчика исполнений по поколению; без performance claim. См. [S3-25c/d report](../reports/STAGE_3_S3-25cd_2026-11-18.md) |
| [partial] | S3-25a | One-vCPU synchronous cache/arena handle lifetime and safe-point checks | Inactive entries only; active-code retirement, SMP, deferred queue not included |
| [partial] | S3-25b | Capacity-two arena lifetime for one V3 artifact and one cached prefix | V3 slot survives cache clear/invalidation; stale source rejected; full retirement/SMP/deferred queue remain open |
| [x] | S3-25c | Normative one-vCPU block lifecycle (four states, safe point, per-transition function and test, production `PagedFetchedLoadSession` declared normative) | One vCPU, current block profile, no linking; rule for future links recorded; SMP, writers outside the session and S3-24 stay open. См. [report](../reports/STAGE_3_S3-25cd_2026-11-18.md) |
| [x] | S3-25d | One deterministic small-capacity (1 and 2 slots) lifecycle stress against the interpreter | Fixture arena only; one vCPU; S3-25 accepted with limitations (Opus gate 2026-11-18). См. [report](../reports/STAGE_3_S3-25cd_2026-11-18.md) |
| [ ] | S3-26 | Порядок side effects и memory classes | S3-19, S3-24 |
| [ ] | S3-27 | Точные fault exits и восстановление state | S3-12, S3-21, S3-26 |
| [x] | S3-28 | STOP/events и безопасные точки — принято с ограничениями (gate-ревью Claude Opus, high, 2026-11-18; закрытие S3-28a–d) | S3-15, S3-16, S3-27; ограничения: один vCPU, JIT-путь без SMP; только cooperative STOP (generated guards + dispatcher), протокол `od-machine` StopAck/SafePoint не заявлен; граница: STOP — одна инструкция (в т.ч. внутри пары), event — один блок (≤32 инструкций по контракту, ≤9–10 через compile pipeline) / одна memory-пара / один bare NOP step; случай k-й инструкции 32-блока — через emitter напрямую; fallback — только bare NOP (REP/сложный fallback в JIT отсутствует; при появлении — только по restart-контракту S2, S3-16/S3-33); `PagedFetchedLoadSession` без STOP/event-входа (STOP — опрос хоста между вызовами, `StopRequested` недостижим по построению), подключение к production host loop — S3-38o; восстановление после fault/host fault — S3-27 (открыт), проверен только приоритет STOP относительно helper/architectural fault; latency S3-28d — локально (Apple M3 Max, один прогон на профиль, register-only формы), не Mini, без обещания миллисекунд; таймеры/APIC/доставка событий не заявлены. См. [S3-28c/d report](../reports/STAGE_3_S3-28cd_2026-11-18.md) |
| [x] | S3-28a | Cooperative event yield на границах register dispatcher | S3-15, S3-16, S3-27 |
| [x] | S3-28b | Cooperative event yield на bounded checked-memory dispatcher | S3-15, S3-20, S3-28a |
| [x] | S3-28c | Сводная матрица STOP/event по путям, граница задержки, общий budget блоков и interpreter | S3-28a, S3-28b |
| [x] | S3-28d | Локальный замер wall-time задержки STOP на busy-пути (не Mini, без обещания миллисекунд) | S3-28c |
| [ ] | S3-29 | Flags по потребности и сравнение стратегий | S3-01, S3-27, S3-28 |
| [x] | S3-29g | Cached NativeRegisterV1 eager/candidate full-dispatch comparison | S3-29f, S3-38b/c |
| [partial] | S3-30 | Ограниченное allocator comparison; выбор по более широким workloads открыт | S3-01, S3-29 |
| [ ] | S3-31 | Прямые связи блоков, если оправданы | S3-25, S3-28, S3-30 |
| [ ] | S3-32 | Локальные IR-преобразования, если оправданы | S3-03, S3-27, S3-30 |
| [ ] | S3-33 | Расширение lowering по готовности S2 | S3-27, S3-29; готовый interpreter/native subset |
| [partial] | S3-33n | Composed uncached `NativeRegisterV1` admission/lowering for committed S3-33a–m forms | Bounded implementation, independent review, and local evidence recorded; 33 local cases `not_run`; broader Stage 3 acceptance remains open |
| [ ] | S3-34 | Corpus, reducer и негативные проверки | S3-24–S3-33 в принятой конфигурации |
| [partial] | S3-34f | Exact local acceptance gates for bounded S3-28/29/38 slices | S3-34e, S3-28a/b, S3-29a–e, S3-38a; full suite gates remain open |
| [partial] | S3-34i | Exact local gates for native arithmetic slices | S3-34e, S3-33a–d; full S3-34 gates remain open |
| [partial] | S3-34j | Add exact local gates for S3-33e CMP imm8 | S3-34i, S3-33e; full S3-34 gates remain open |
| [partial] | S3-34k | Exact local gates for S3-33i XCHG RAX,RBX | S3-34j, S3-33i; full S3-34 gates remain open |
| [partial] | S3-34l | Exact local gates for S3-33j LEA RAX,[RBX+disp8] | S3-34k, S3-33j; full S3-34 gates remain open |
| [partial] | S3-34p | Cached NativeRegisterV1 corpus and versioned replay | S3-34n, S3-38b/c; full S3-34 gates remain open |
| [partial] | S3-34u | Exact local harness gates for committed dword-load and single-site PC-map tests | Four exact Rust test identities; integration target and source fingerprints pinned; corpus/reducer and full S3-34 remain open. See [report](../reports/STAGE_3_S3-34u_2026-11-01.md) |
| [partial] | S3-34v | Exact local harness gates for committed dword-store admission and host behavior tests | Two exact Rust test identities; Cargo invocation and source fingerprints remain pinned; broader store coverage, corpus/reducer and full S3-34 remain open. See [report](../reports/STAGE_3_S3-34v_2026-11-01.md) |
| [partial] | S3-34w | Exact local harness gates for committed dword load/store PC-map contracts and publication lifecycle | Three exact Rust test identities; `od-jit --lib` and `od-host --test memory_corpus` targets pinned with manifest/source fingerprints; broader corpus/reducer and full S3-34 remain open. See [report](../reports/STAGE_3_S3-34w_2026-11-01.md) |
| [partial] | S3-34x | Exact local harness gates for committed dword-alias load admission, PC-map, and host behavior | Three exact library test identities (`od-jit --lib` ×2, `od-host --lib` ×1) with required manifest and source fingerprints; Mini, M1, Stage 2, corpus/reducer, and full S3-34 remain open |
| [partial] | S3-34y | Exact local harness gates for committed dword-alias store admission, PC-map, and host behavior | Three exact library test identities (`od-jit --lib` ×2, `od-host --lib` ×1) with required manifest and source fingerprints; Mini, M1, Stage 2, corpus/reducer, and full S3-34 remain open |
| [partial] | S3-34z | Exact local harness gates for committed RBX+disp8 dword-load source binding and host behavior | Two exact library test identities (`od-jit --lib`, `od-host --lib`) with manifest/source fingerprints and fail-closed evidence; integrated main after `9a8e5c5`: focused harness 47/47 and CPU unittest discovery 154/154; S3-34u retains the restored exact `s3_26c_ir_dword_load_zero_extends_and_reads_exactly_four_bytes` (`8B 03`) identity. Mini, M1, Stage 2, corpus/reducer, and full S3-34 remain open. See [report](../reports/STAGE_3_S3-34z_2026-11-01.md) |
| [partial] | S3-34aa | Exact local harness gates for committed RBX+disp32 dword-load source binding, PC map, and host behavior | Three exact library identities (`od-jit --lib` ×2, `od-host --lib` ×1) with manifest/source fingerprints and fail-closed evidence; after `e919e61`, integrated harness 48/48 and CPU discovery 155/155. Existing S3-34z disp8 host gate remains pinned; corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34aa_2026-11-01.md) |
| [partial] | S3-34ab | Exact local harness gates for the RBX+disp8 dword store | Three exact library identities (`od-jit --lib` ×2, `od-host --lib` ×1) with manifest/source fingerprints and fail-closed evidence; integrated harness 49/49 and CPU discovery 156/156. Corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34ab_2026-11-01.md) |
| [partial] | S3-34ac | Exact harness integration gate for published disp8/disp32 dword-load PC maps | Pins the exact `od-host --test memory_corpus` identity for S3-27h with target, invocation, and source fingerprints; integration test passed 1/1, harness 50/50 and CPU discovery 157/157. Does not close corpus/reducer or full S3-34. См. [report](../reports/STAGE_3_S3-34ac_2026-11-01.md) |
| [partial] | S3-34ad | Exact harness gates for disp32 dword-store source binding, PC map, and host semantics | Pins two S3-20cu JIT identities and its host checked-semantics identity; current integrated main after `d9c268a` has harness 52/52 and CPU discovery 159/159. Broader corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34ad_2026-11-01.md) |
| [partial] | S3-34ae | Exact harness gate for S3-27i disp32 dword-store canonical-end/range-overflow regression | Pins `tests::s3_27i_disp32_dword_store_checks_canonical_end_and_range_overflow` in `od-host --lib`; harness 52/52 and CPU discovery 159/159 after `d9c268a`. Does not close corpus/reducer or full S3-34. См. [report](../reports/STAGE_3_S3-34ae_2026-11-01.md) |
| [partial] | S3-34af | Exact harness integration gate for published disp8/disp32 dword-store PC maps | Pins `published_dword_disp8_and_disp32_store_pc_maps_match_artifacts_survive_drop_and_republish` in `od-host --test memory_corpus`; integrated test passed 1/1 and harness 53/53 / CPU discovery 160/160 after `30a516b`. Does not close corpus/reducer or full S3-34. См. [report](../reports/STAGE_3_S3-34af_2026-11-01.md) |
| [partial] | S3-34ag | Exact local harness gates for RIP-relative disp32 dword-load source binding, PC map, and guest NextRip | Pins two JIT identities and the S3-20cv host identity; integrated harness 54/54 and CPU discovery 161/161 after `7639d98`. No new S3-20cw gate is claimed. Broader corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34ag_2026-11-01.md) |
| [partial] | S3-34ah | Exact local harness gates for RIP-relative disp32 dword-store source binding, PC map, and guest NextRip | Pins the three S3-20cw identities from runtime commit `7639d98`; harness slice commit `1708836`. Current integrated evidence after `629e144`: harness 55/55 and CPU discovery 162/162. Broader corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34ah_2026-11-02.md) |
| [partial] | S3-34ai | Exact local harness gates for address-size-32 dword-load admission, PC map, and host behavior | Pins the two S3-20cx JIT identities and exact host identity; integrated harness 56/56 and CPU discovery 163/163 after `2ee1955`. Broader corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34ai_2026-11-02.md) |
| [partial] | S3-34aj | Exact local harness gates for address-size-32 dword-store admission and host behavior | Pins the S3-20cy exact admission/provenance JIT identity and host behavior identity; no PC-map gate is claimed. Integrated main after runtime commit `0f1dab2` (harness commit `7d7bf59`): harness 57/57 and CPU discovery 164/164. Broader corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34aj_2026-11-02.md) |
| [partial] | S3-34ak | Exact local harness gates for S3-20cz addr32 disp8 load source/site identity and wrapped-EBX host behavior | Pins `ir_dword_addr32_disp8_load_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20cz_addr32_disp8_dword_load_uses_wrapped_ebx`; no PC-map gate. Integrated main after `10b0f08` / `ce13e6e`: harness 58/58 and CPU discovery 165/165. Broader corpus/reducer and full S3-34 remain open. См. [report](../reports/STAGE_3_S3-34ak_2026-11-02.md) |
| [partial] | S3-34al | Exact harness gates for S3-20da addr32 disp32 load source/site identity and wrapped-EBX host behavior | Harness commit `2751ba2` pins the S3-20da JIT source/site and host identities; no PC-map gate. Both exact Cargo gates passed 1/1; focused harness self-test passed, and full Stage 3 harness passed 59/59. No CPU discovery was run for this slice. См. [report](../reports/STAGE_3_S3-34al_2026-11-02.md) |
| [partial] | S3-34am | Exact harness gates for S3-20db addr32 disp8 dword-store admission and host behavior | Harness commit `c194891` pins only `ir_dword_addr32_disp8_store_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20db_addr32_disp8_dword_store_uses_wrapped_ebx`; no PC-map gate. Exact Cargo gates and focused harness self-test passed 1/1; full Stage 3 harness passed 60/60. No CPU discovery was run for this slice. См. [report](../reports/STAGE_3_S3-34am_2026-11-02.md) |
| [partial] | S3-34an | Exact harness gates for S3-20dc addr32 disp32 dword-store source/site identity and wrapped-EBX host behavior | Harness commit `11bf23b` pins only `memory_load_v2::tests::ir_dword_addr32_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20dc_addr32_disp32_dword_store_uses_wrapped_ebx`; no PC-map gate. Both exact Cargo gates and the focused harness self-test passed 1/1. The full Stage 3 harness/module and CPU discovery were not run for this slice. См. [report](../reports/STAGE_3_S3-34an_2026-11-02.md) |
| [partial] | S3-34ao | Exact harness gates for S3-20dd address64 SIB dword load admission and host behavior | Harness commit `d3d2c05` pins exactly `memory_load_v2::tests::ir_dword_rbx_rcx_sib_load_admits_only_exact_8b040b` and `tests::s3_20dd_rbx_rcx_sib_dword_load_uses_full_width_base_and_index`; no PC-map gate. Both exact Cargo gates and the focused harness self-test passed 1/1. The full Stage 3 harness/module and CPU discovery were not run for this slice. См. [report](../reports/STAGE_3_S3-34ao_2026-11-02.md) |
| [partial] | S3-34ap | Exact harness gates for S3-20de address64 SIB dword-store admission and host behavior | Harness commit `f767bd9` pins exactly `memory_load_v2::tests::ir_dword_rbx_rcx_sib_store_admits_only_exact_89040b` and `tests::s3_20de_rbx_rcx_sib_dword_store_uses_full_width_base_and_index`; no PC-map gate. Both exact Cargo gates and the focused harness self-test passed 1/1. The full Stage 3 harness/module and CPU discovery were not run for this slice. См. [report](../reports/STAGE_3_S3-34ap_2026-11-02.md) |
| [partial] | S3-34aq | Exact harness gates for S3-20df address64 SIB dword-load scale 2/4/8 source admission and host behavior | Pins exactly `memory_load_v2::tests::ir_dword_rbx_rcx_sib_dword_load_binds_exact_scales_2_4_8` and `tests::s3_20df_rbx_rcx_sib_scaled_dword_load_uses_full_width_base_and_index`; no PC-map gate. Both exact Cargo gates and focused harness self-test passed 1/1. Full Stage 3 harness/module and CPU discovery were not run. S3-34ao remains the separate S3-20dd scale-1 gate. См. [report](../reports/STAGE_3_S3-34aq_2026-11-02.md) |
| [partial] | S3-34ar | Exact harness gates for S3-20dg address64 SIB dword-store scales 2/4/8 source admission and host behavior | Harness commit `5ab8dca` pins exactly `memory_load_v2::tests::ir_dword_rbx_rcx_sib_store_binds_exact_scales_2_4_8` and `tests::s3_20dg_rbx_rcx_scaled_sib_dword_store_uses_full_width_base_and_index`; no PC-map gate. Both exact Cargo gates and focused harness self-test passed 1/1. Full Stage 3 harness and CPU discovery were not run. S3-34ap remains the S3-20de scale-1 store gate; S3-34aq remains the S3-20df load gate. См. [report](../reports/STAGE_3_S3-34ar_2026-11-02.md) |
| [partial] | S3-34as | Exact harness gates for S3-20dh address64 SIB disp8 dword-load scales/source binding and host semantics | Pins exactly `memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp8_load_binds_all_scales_and_signed_bytes` and `tests::s3_20dh_rbx_rcx_scaled_disp8_sib_dword_load_uses_signed_displacement`; no PC-map gate. Both exact Cargo gates and focused harness self-test passed 1/1. Full harness/module and CPU discovery were not run. Prior scale-only gates remain separate. См. [report](../reports/STAGE_3_S3-34as_2026-11-02.md) |
| [partial] | S3-34au | Exact harness gates for S3-20dj address64 SIB disp32 dword-load source/site binding and host behavior | Harness commit `4ef90b0` pins exactly `memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp32_load_binds_all_scales_and_source_bytes` and `tests::s3_20dj_rbx_rcx_scaled_disp32_sib_dword_load_preserves_checked_semantics`. Both exact Cargo gates and the focused harness self-test passed 1/1; no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify query/update passed. Full harness/module and CPU discovery were not run; earlier scale-only S3-34ao/aq and disp8 S3-34as gates remain separate. См. [report](../reports/STAGE_3_S3-34au_2026-11-02.md) |
| [partial] | S3-34av | Exact harness gates for S3-20dk address64 SIB disp32 dword-store source/site binding and host behavior | Harness commit `27dba0e` pins exactly `memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp32_store_binds_all_scales_and_source_bytes` and `tests::s3_20dk_rbx_rcx_scaled_disp32_sib_dword_store_preserves_checked_semantics`. Both Cargo gates and the focused harness self-test passed 1/1; no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify query/update passed. Full harness/module and CPU discovery were not run; prior scale-only and disp8 SIB gates remain separate. См. [report](../reports/STAGE_3_S3-34av_2026-11-02.md) |
| [partial] | S3-34aw | Exact harness gates for S3-20dl addr32 SIB dword-load source/site identity and host behavior | Harness commit `62a1332` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_load_binds_exact_scales_and_prefix` and `tests::s3_20dl_addr32_scaled_sib_dword_load_wraps_and_zero_extends`. Both exact Cargo gates and the focused harness self-test passed 1/1; no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full harness/module and CPU discovery were not run; earlier S3-34ai/ak/al addr32 load gates remain separate. См. [report](../reports/STAGE_3_S3-34aw_2026-11-02.md) |
| [partial] | S3-34ax | Exact validator gates for S3-20dm address-size-32 SIB dword store | Harness commit `0d33332` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_store_binds_exact_scales_and_prefix` and `tests::s3_20dm_addr32_scaled_sib_dword_store_wraps_and_preserves_state`; both Cargo gates passed 1/1 and the focused self-test passed. Python validator module passed 71/71; aggregate counters: 170 gates, 160 acceptance, 10 lifecycle, 188 steps. No PC-map gate; the full Stage 3 harness, CPU discovery, and full hardware acceptance were not run. См. [report](../reports/STAGE_3_S3-34ax_2026-11-02.md) |
| [partial] | S3-34ay | Exact harness gates for S3-20dn addr32 SIB disp8 dword-load source/site binding and host semantics | Integrated harness commit `ed413cd` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp8_load_binds_all_scales_and_signed_bytes` and `tests::s3_20dn_addr32_scaled_disp8_sib_dword_load_uses_signed_displacement`; both Cargo gates and focused self-test passed 1/1. No PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full Python module counts and CPU discovery were not reported; no full-module or hardware acceptance claim. См. [report](../reports/STAGE_3_S3-34ay_2026-11-02.md) |
| [partial] | S3-34az | Exact harness gates for S3-20do addr32 SIB disp8 dword-store source/site binding and wrapped-state host behavior | Integrated harness commit `c7d4e9b` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp8_store_binds_all_scales_and_signed_bytes` and `tests::s3_20do_addr32_scaled_disp8_sib_dword_store_wraps_and_preserves_state`; both exact Cargo gates and the focused self-test passed 1/1. No PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full Python module, CPU discovery, and hardware acceptance were not reported. Earlier S3-34aw/ax/ay gates remain distinct. См. [report](../reports/STAGE_3_S3-34az_2026-11-02.md) |
| [partial] | S3-34ba | Exact harness gates for S3-20dp addr32 SIB disp32 dword-load source/site identity and host behavior | Integrated harness commit `4ef1475` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp32_load_binds_scales_and_all_displacement_bytes` and `tests::s3_20dp_addr32_scaled_disp32_sib_dword_load_wraps_and_preserves_state`; both exact Cargo gates and the focused fail-closed self-test passed 1/1. No PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full Python module, CPU discovery, Mini, M1, and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-34ba_2026-11-02.md) |
| [partial] | S3-34bb | Exact harness gates for S3-20dq addr32 SIB disp32 dword-store source/site identity and host behavior | Integrated harness commit `6e7ea49` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp32_store_binds_scales_and_all_displacement_bytes` and `tests::s3_20dq_addr32_scaled_disp32_sib_dword_store_wraps_and_preserves_state`. Both exact Cargo gates passed 1/1; three focused Python self-tests passed 3/3, including rejection of Cargo output containing `running 0 tests`. Python byte-compilation, Graphify update, and diff-check passed; Sol harness review passed. No PC-map gate. No CPU suite or full Python module was run; Mini, M1, and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-34bb_2026-11-02.md) |
| [partial] | S3-34bc | Exact harness gates for S3-20dr RBP disp8 dword-load source/site identity and host checked semantics | Harness commit `ceb418c` pins exactly `memory_load_v2::tests::ir_dword_rbp_disp8_load_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20dr_rbp_disp8_dword_load_preserves_ss_fault_and_checked_state`, with `od-jit` and `od-host` `--lib --exact` targets. Both exact Rust gates passed 1/1; four focused Python self-tests passed 4/4, covering mandatory gate pinning, zero-match rejection, and missing-gate manifest/summary cases. Python byte-compilation, Graphify update, diff-check, and Sol harness review passed. No PC-map gate; full `od-host`, full Python module, CPU suite, Mini, M1, and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-34bc_2026-11-02.md) |
| [partial] | S3-34bd | Exact harness gates for S3-20ds RBP disp8 dword-store source/site identity and host checked semantics | Harness commit `7e28b8e` pins exactly `memory_load_v2::tests::ir_dword_rbp_disp8_store_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20ds_rbp_disp8_dword_store_preserves_ss_fault_and_checked_state` in `od-jit` and `od-host` `--lib --exact` targets. Both exact Rust gates passed 1/1; the Python harness validation module passed 77/77. Python byte-compilation, Graphify update, diff-check, and Sol harness review passed. No separate PC-map gate. Full `od-host`, Mini, M1, and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-34bd_2026-11-02.md) |
| [partial] | S3-34be | Exact harness gates for S3-20dt RBP disp32 dword-load source/site identity and host checked semantics | Harness commit `40b4b8e` pins `memory_load_v2::tests::ir_dword_rbp_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20dt_rbp_disp32_dword_load_preserves_ss_fault_and_checked_state` in their `od-jit`/`od-host` `--lib --exact` targets. Both exact Rust gates passed 1/1; the full Python harness validation module passed 78/78. Python byte-compilation, Graphify update, diff-check, and independent Sol/high review passed. The source/site test verifies displacement-byte binding, not an exhaustive 2^32 displacement sweep. No separate PC-map gate is pinned. Full `od-host`, Mini, M1, and Stage 2 were not run. См. [report](../reports/STAGE_3_S3-34be_2026-11-02.md) |
| [partial] | S3-34bf | Exact harness gates for S3-20du RBP disp32 dword-store source/site identity and host checked semantics | Harness commit `439ce43` pins exactly `memory_load_v2::tests::ir_dword_rbp_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20du_rbp_disp32_dword_store_preserves_ss_fault_and_checked_state` in their `od-jit`/`od-host` `--lib --exact` targets. Both exact Rust gates passed 1/1; the Python harness validation module passed 79/79. Python byte-compilation, Graphify update, diff-check, and Sol review passed. The source/site gate checks the runtime test's representative disp32 samples and byte mutations, not an exhaustive 2^32 sweep. No separate PC-map gate. Full `od-host`, Mini, M1, and Stage 2 were not run. Overall S3-34 and Stage 3 remain partial/open. См. [report](../reports/STAGE_3_S3-34bf_2026-11-02.md) |
| [partial] | S3-34bg | Exact harness gates for S3-20dv addr32 RBP disp8 dword load | Harness commit `40876a7` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbp_disp8_load_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20dv_addr32_rbp_disp8_dword_load_preserves_checked_state` in the `od-jit` and `od-host` `--lib --exact` targets. Both exact gates and the focused mandatory self-test passed 1/1; the full Python harness validation module passed 80/80. The JIT gate binds all 256 encoded disp8 bytes; host coverage checks signed edges across low/high RBP values, not 256 host cases. Python byte-compilation, Graphify update, diff-check, and Sol review passed. No separate PC-map gate or address-size-64 `#SS` claim. Full `od-host`, Mini, M1, and Stage 2 were not run. Overall S3-34 and Stage 3 remain partial/open. См. [report](../reports/STAGE_3_S3-34bg_2026-11-02.md) |
| [partial] | S3-34bh | Exact harness gates for S3-20dw addr32 RBP disp8 dword store | Harness commit `0cff500` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbp_disp8_store_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20dw_addr32_rbp_disp8_dword_store_uses_wrapped_ebp` in their `od-jit`/`od-host` `--lib --exact` targets. Both Rust gates passed 1/1; the mandatory focused self-test passed 1/1; the full Python harness validation module passed 81/81. The reviewer’s four related cross-slice gate checks passed 4/4, covering S3-34bh and the existing S3-34bg, S3-34bd, and S3-34bf gate validations. The manifest contains 190 gates (180 acceptance, 10 lifecycle) and 208 local steps. Python byte-compilation, Graphify update, diff-check, and Sol review passed. No separate PC-map gate or address-size-64 `#SS` claim. Full `od-host`, Mini, M1, and Stage 2 were not run. Overall S3-34 and Stage 3 remain partial/open. См. [report](../reports/STAGE_3_S3-34bh_2026-11-02.md) |
| [partial] | S3-34bi | Exact harness gates for S3-20dx addr32 RBP disp32 dword load | Harness commit `a0919e6` pins `memory_load_v2::tests::ir_dword_addr32_rbp_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20dx_addr32_rbp_disp32_dword_load_preserves_checked_state` as `od-jit` and `od-host` `--lib --exact` gates. Both Rust gates passed 1/1; the focused fail-closed Python test passed 1/1; the full Python Stage 3 harness module passed 82/82. The manifest now contains 192 gates (182 acceptance, 10 lifecycle) and 210 local steps. Python byte-compilation, Graphify update, diff-check, and Sol review passed. No separate PC-map gate. Full `od-host`, Mini, M1, and Stage 2 were not run. S3-34 and Stage 3 remain partial/open. См. [JIT testing reference](../testing/JIT.md#s3-34bi-exact-harness-gates-for-s3-20dx-address-size-32-rbp-disp32-dword-load) и [report](../reports/STAGE_3_S3-34bi_2026-11-02.md) |
| [partial] | S3-34bj | Exact harness gates for S3-20dy addr32 RBP disp32 dword store | Harness commit `f3a2074` pins exactly `memory_load_v2::tests::ir_dword_addr32_rbp_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20dy_addr32_rbp_disp32_dword_store_uses_wrapped_ebp` in their `od-jit`/`od-host` `--lib --exact` targets. Both exact Rust gates passed 1/1; the mandatory focused self-test passed 1/1; the full Python Stage 3 harness module passed 83/83. Python byte-compilation, Graphify update, diff-check, and harness Sol review passed. The JIT identity pins source/site coverage for the disp32 test; this does not claim an exhaustive 2^32 sweep. No separate PC-map gate or address-size-64 RBP `#SS` claim. Full `od-host`, Mini, M1, and Stage 2 were not run. S3-34 and Stage 3 remain partial/open. См. [JIT testing reference](../testing/JIT.md#s3-34bj-exact-harness-gates-for-s3-20dy-address-size-32-rbp-disp32-dword-store) и [report](../reports/STAGE_3_S3-34bj_2026-11-02.md) |
| [partial] | S3-34bk | Exact harness gates for S3-20dz RSP SIB dword load | Harness commit `84a96af` pins `memory_load_v2::tests::ir_dword_rsp_sib_load_binds_exact_source_and_site` and `tests::s3_20dz_rsp_dword_load_preserves_ss_fault_and_checked_state` as `od-jit`/`od-host` `--lib --exact` gates. Both Rust gates passed 1/1; the focused fail-closed harness self-test passed 1/1, and the full Python module passed 84/84. The self-test requires host step `s3_34bk_host_rsp_dword_load_no_replay_retry_checked_state` in the final local-acceptance summary and rejects a summary with that host identity omitted. The earlier S3-34bh JIT summary-membership negative remains in place; S3-34bk adds the corresponding host no-replay omission check. Gate tuples and wrong-identity assertions are unchanged. Manifest totals: 196 Rust gates (186 acceptance, 10 lifecycle), 214 local steps. Python byte-compilation, Graphify update, diff-check, and Sol review passed. No separate PC-map phase claim. Full `od-host`, Mini, M1, and Stage 2 were not run; S3-34 and Stage 3 remain partial/open. См. [JIT testing reference](../testing/JIT.md#s3-34bk-exact-harness-gates-for-s3-20dz-rsp-sib-dword-load) и [report](../reports/STAGE_3_S3-34bk_2026-11-02.md) |
| [partial] | S3-34bl | Exact harness gates for S3-20ea RSP SIB dword store | Harness commit `eea1d78` pins `memory_load_v2::tests::ir_dword_rsp_sib_store_binds_exact_source_and_site` and `tests::s3_20ea_rsp_dword_store_preserves_ss_fault_and_checked_state` as exact `od-jit`/`od-host --lib` gates. Both Rust gates passed 1/1; mandatory fail-closed self-test `test_s3_34bl_rsp_dword_store_gates_are_pinned_and_fail_closed` passed 1/1; full Python Stage 3 harness module passed 85/85. Self-test checks gate registry and manifest/source fingerprints, exact argv and required evidence, rejects missing manifest/evidence and wrong, ignored, failed, or zero-match output, and requires the host step in final local-acceptance summary. Manifest totals: 198 Rust gates (188 acceptance, 10 lifecycle), 216 local steps. Python byte-compilation, Graphify update, diff-check, and Sol review passed. No separate PC-map gate. Full `od-host`, Mini, M1, and Stage 2 were not run; S3-34 and Stage 3 remain partial/open. См. [JIT testing reference](../testing/JIT.md#s3-34bl-exact-harness-gates-for-s3-20ea-rsp-sib-dword-store) и [report](../reports/STAGE_3_S3-34bl_2026-11-02.md) |
| [partial] | S3-34bm | Exact harness gates for S3-20eb RSP disp8 dword load | Harness commit `ae2a943` pins `memory_load_v2::tests::ir_dword_rsp_sib_disp8_load_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20eb_rsp_disp8_dword_load_preserves_ss_fault_and_checked_state` as exact `od-jit`/`od-host --lib` gates. Both Rust gates passed 1/1; mandatory fail-closed self-test `test_s3_34bm_rsp_disp8_dword_load_gates_are_pinned_and_fail_closed` passed 1/1; full Python harness validation module passed 86/86. The self-test pins manifest/source evidence and exact argv, rejects absent/zero-match/ignored/failed/wrong-identity results, and requires the host no-replay step in the final local-acceptance summary. Manifest totals: 200 gates (190 acceptance, 10 lifecycle), 218 local steps. Python byte-compilation, Graphify update, diff-check, and independent Sol review passed. No separate PC-map gate. Full `od-host`, Mini, M1, and Stage 2 were not run; S3-34 and Stage 3 remain partial/open. См. [JIT testing reference](../testing/JIT.md#s3-34bm-exact-harness-gates-for-s3-20eb-rsp-disp8-dword-load) и [report](../reports/STAGE_3_S3-34bm_2026-11-02.md) |
| [partial] | S3-34bn | Exact harness gates for S3-20ec RSP disp8 dword store | Harness commit `2edeb07` pins exactly the S3-20ec JIT source/site identity and host checked-state identity as `od-jit` / `od-host --lib --exact` gates. On integrated revision `2edeb07`, both Rust gates passed 1/1 and the full Python harness validation module passed 87/87. The mandatory self-test pins each tuple once in the gate registry/manifest, checks exact argv and source identity, rejects missing gate/evidence, zero-match, ignored, failed, and wrong-identity output, and rejects a final summary omitting the host gate. No PC-map claim. Full crate counts are recorded in the [S3-20ec runtime report](../reports/STAGE_3_S3-20ec_2026-11-02.md); Mini/M1 remain deferred and Stage 3 remains partial. См. [JIT testing reference](../testing/JIT.md#s3-34bn-exact-harness-gates-for-s3-20ec-rsp-disp8-dword-store) и [report](../reports/STAGE_3_S3-34bn_2026-11-02.md) |
| [partial] | S3-34bo | Exact harness gates for S3-20ed RSP disp32 dword load | Harness commit `ed9f45e` pins exactly the S3-20ed JIT source/site and host checked-state identities as `od-jit` / `od-host --lib --exact` gates. On integrated revision `238799d`, both exact Cargo gates passed 1/1 and the full Python Stage 3 harness passed 88/88. The mandatory self-test pins each tuple once in the registry and manifest, checks source identity and exact argv, rejects missing gate or summary evidence, zero-match, ignored, failed, and wrong-identity output, and rejects a final summary omitting the host gate. This is gate-evidence only; no full `od-host` or separate PC-map claim. Runtime crate evidence is in the [S3-20ed report](../reports/STAGE_3_S3-20ed_2026-11-02.md). Mini/M1 remain deferred and Stage 3 remains partial. См. [JIT testing reference](../testing/JIT.md#s3-34bo-exact-harness-gates-for-s3-20ed-rsp-disp32-dword-load) и [report](../reports/STAGE_3_S3-34bo_2026-11-02.md) |
| [partial] | S3-34bq | Exact harness gates for S3-20ef address32 ESP-base dword load | Harness code commit `28cd043`, integrated as `1ba352a`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rsp_sib_load_binds_exact_source_and_site` and `tests::s3_20ef_addr32_rsp_dword_load_uses_zero_extended_esp` as `od-jit`/`od-host --lib --exact` gates. At root revision `a0ce871`, after runtime `392886b` was integrated and 34br harness commit `ac069ca` was present, both exact gates passed 1/1 and the full Python Stage 3 harness passed 91/91; the module count is cumulative and includes 34br. The fail-closed self-test checks gate registries/manifest, source fingerprints and exact argv; it rejects missing gate/evidence, zero-match, ignored, failed, wrong-identity output, and a final summary omitting the host gate. No full `od-host` claim. See the [S3-20ef runtime report](../reports/STAGE_3_S3-20ef_2026-11-02.md) for runtime semantics and [S3-34bq report](../reports/STAGE_3_S3-34bq_2026-11-02.md) for harness evidence. Mini/M1 remain deferred; Stage 3 remains partial. См. [JIT testing reference](../testing/JIT.md#s3-34bq-exact-harness-gates-for-s3-20ef-address32-esp-base-dword-load) |
| [partial] | S3-34br | Exact harness gates for S3-20eg address32 ESP-base dword store | Harness source commit `08e8ae2`, integrated as `ac069ca`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rsp_sib_store_binds_exact_source_and_site` and `tests::s3_20eg_addr32_rsp_dword_store_uses_zero_extended_esp` as exact `od-jit` / `od-host --lib` gates. Runtime source `abd2af4` is integrated as `8052314`; at that integrated root both exact Cargo gates passed 1/1 and the full Python Stage 3 harness passed 91/91 (cumulative with 34bq/34br). The fail-closed self-test verifies the pinned identities and gate evidence, rejecting absent/zero-match/ignored/failed/wrong-identity output and a final summary that omits the host gate. Manifest totals: 210 Rust gates (200 acceptance, 10 lifecycle), 228 local steps. Sol harness review passed. The updated S3-20eg host gate and this fail-closed self-test now pass 1/1, closing ISSUE-045 only for the retry assertions. No full `od-host` or separate PC-map claim. Stage 2 remained outside this slice’s evidence; its concurrent runs were ignored per user instruction. Mini SSH to `senya@192.168.1.14:22` timed out before remote commands/tests; M1 remains deferred; Stage 3 remains partial. См. [JIT testing reference](../testing/JIT.md#s3-34br-exact-harness-gates-for-s3-20eg-address32-esp-base-dword-store) и [report](../reports/STAGE_3_S3-34br_2026-11-02.md) |
| [partial] | S3-34bs | Exact harness gates for S3-20eh address32 ESP-base disp8 dword load | Harness source commit `e2befb4`, integrated as `046f5a2`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp8_load_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20eh_addr32_rsp_disp8_dword_load_uses_wrapped_esp` as `od-jit` / `od-host --lib --exact` gates. Runtime source `62a516a` is integrated as `08b3eba`; at that integrated root both exact Rust gates passed 1/1 and the full Python Stage 3 harness passed 92/92 (cumulative before later slices). The fail-closed self-test checks pinned registries/manifest, exact gate evidence and summary membership; it rejects missing, zero-match, ignored, failed, or wrong-identity output and a final summary that omits the host gate. Manifest totals: 212 gates (202 acceptance, 10 lifecycle), 230 local steps. Harness review passed Sol. No full `od-host` or separate PC-map claim. See [S3-20eh runtime report](../reports/STAGE_3_S3-20eh_2026-11-02.md) for runtime results and [S3-34bs report](../reports/STAGE_3_S3-34bs_2026-11-02.md) for harness evidence. Stage 2 remained outside this slice’s evidence; its concurrent runs were ignored per user instruction. Mini/M1 are deferred; Stage 3 remains partial. См. [JIT testing reference](../testing/JIT.md#s3-34bs-exact-harness-gates-for-s3-20eh-address32-esp-base-disp8-dword-load) |
| [partial] | S3-34bu | Exact harness gates for S3-20ej address32 ESP-base disp32 dword load | Harness source commit `00430e9`, integrated as `4790e76`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20ej_addr32_rsp_disp32_dword_load_uses_wrapped_esp` as exact `od-jit` / `od-host --lib` gates. Runtime source `d649bd4` integrated as `4e4fec2`; at root revision `4e4fec2` both exact gates passed 1/1 and the full Python Stage 3 harness passed 94/94. The harness author branch separately reported 95/95; this is distinct from, and not substituted for, the verified integrated-root count. The fail-closed self-test verifies the two exact identities in registries/manifest, source fingerprints, Cargo argv, evidence and summary membership; missing gates/evidence, zero-match/ignored/failed/wrong-identity output, or omission of the host gate are rejected. Manifest totals: 216 Rust gates (206 acceptance, 10 lifecycle), 234 local steps. Sol harness review passed. No full `od-host` or separate PC-map claim. See [S3-20ej runtime report](../reports/STAGE_3_S3-20ej_2026-11-02.md) and [S3-34bu report](../reports/STAGE_3_S3-34bu_2026-11-02.md). Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. См. [JIT testing reference](../testing/JIT.md#s3-34bu-exact-harness-gates-for-s3-20ej-address32-esp-base-disp32-dword-load) |
| [partial] | S3-34bv | Exact harness gates for S3-20ek address32 ESP-base disp32 dword store | Harness source commit `7b1651f`, integrated as `60f9e7a`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20ek_addr32_rsp_disp32_dword_store_uses_wrapped_esp` as exact `od-jit` / `od-host --lib` local-acceptance gates. Runtime source `34ad7e2` is integrated as `190a61c`. At root `b2f6bd8`, the exact JIT gate passed 1/1; the host gate remains pending under the user's Mini deferral and is not claimed as passing. The author's worktree Python suite passed 95/95; the root Python session ended before a final result, so no root full-suite count is claimed. The fail-closed self-test pins exact gate identities once in both registries and the manifest, checks source fingerprints and exact Cargo argv, rejects a changed source pin, missing gates/evidence, zero-match/ignored/failed/wrong-identity output, and a final summary omitting the host gate. Manifest totals: 218 Rust gates (208 acceptance, 10 lifecycle), 236 local steps. No full `od-host` or separate PC-map claim. Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. См. [S3-20ek runtime report](../reports/STAGE_3_S3-20ek_2026-11-02.md), [S3-34bv report](../reports/STAGE_3_S3-34bv_2026-11-02.md) и [JIT testing reference](../testing/JIT.md#s3-34bv-exact-harness-gates-for-s3-20ek-address32-esp-base-disp32-dword-store) |
| [partial] | S3-34bw | Exact harness gates for S3-20el absolute SIB disp32 dword load | Harness source `4dc35bd4b6308943dd62055e2a8fdb2164c67872`, integrated as `9f5ea21`, pins exactly `memory_load_v2::tests::ir_dword_absolute_sib_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20el_absolute_sib_disp32_dword_load_uses_signed_absolute_address` as `od-jit` / `od-host --lib` exact local-acceptance gates. The author worktree Python validation module passed 96/96. Its fail-closed self-test verifies the two identities in both gate registries and the manifest, exact counts and Cargo argv, source fingerprints, and evidence/summary membership; it rejects source-pin changes, missing gates/evidence, zero-match, ignored, failed, or wrong-identity output, and a summary omitting the host gate. Manifest totals: 220 Rust gates (210 acceptance, 10 lifecycle), 238 local steps. At the harness authoring/integration revision `9f5ea21`, S3-20el runtime was not yet integrated and neither exact Cargo gate was run as part of this slice. Runtime source `aa8e38f` was integrated later as `e564943`; no runtime test result or behavior claim belongs to this harness report. Sol/high harness review passed with no findings. No root full-suite, full `od-host`, Mini/M1, or Stage 2 result is claimed. Stage 2 remained outside this slice's evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. См. [report](../reports/STAGE_3_S3-34bw_2026-11-02.md) и [JIT testing reference](../testing/JIT.md#s3-34bw-exact-harness-gates-for-s3-20el-absolute-sib-disp32-dword-load) |
| [partial] | S3-34bx | Exact harness identities for S3-20em absolute SIB disp32 Dword store | Harness source commit `b165126c83378cbc69168f5201dc3ddabd340d48`, integrated as `a70a022`, pins exactly `memory_load_v2::tests::ir_dword_absolute_sib_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20em_absolute_sib_disp32_dword_store_uses_signed_absolute_address` as exact JIT/host acceptance gates. Static manifest totals: 222 Rust gates (212 acceptance, 10 lifecycle), 240 local steps. Independent Sol/high harness review passed. No Python, Cargo, or runtime gates were run for this slice under the Mini deferral. Runtime source was integrated later as `5d8e9ac`; no Cargo gate result is claimed here. ISSUE-045 remains open; Stage 3 remains partial. См. [JIT testing reference](../testing/JIT.md#s3-34bx-exact-harness-gates-for-s3-20em-absolute-sib-disp32-dword-store) и [report](../reports/STAGE_3_S3-34bx_2026-11-02.md) |
| [partial] | S3-34bz | Exact harness gates for S3-20eo addr32 absolute SIB Dword store | Harness source `7ee99dc`, integrated as `5069141`, pins exactly `memory_load_v2::tests::ir_dword_addr32_absolute_sib_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20eo_addr32_absolute_sib_disp32_dword_store_uses_zero_extended_absolute_address` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 226 Rust gates (216 acceptance, 10 lifecycle), 244 local steps; independent Sol/high review passed. No Python harness validation or Cargo gate ran. Runtime `c98a28d` is integrated as `4970c5c` and passed static review after coverage fixes, but no runtime tests/build/fmt/Cargo/Python checks ran, so no runtime evidence is claimed. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34bz-exact-harness-gates-for-s3-20eo-addr32-absolute-sib-dword-store) и [report](../reports/STAGE_3_S3-34bz_2026-11-02.md) |
| [partial] | S3-34ca | Exact harness gates for S3-20ep RCX-indexed absolute SIB Dword load | Harness source `8b05d2f`, integrated as `02c92fa`, pins exactly `memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20ep_rcx_index_no_base_sib_disp32_dword_load_uses_signed_displacement` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 228 Rust gates (218 acceptance, 10 lifecycle), 246 local steps; independent Sol/high static review PASS. No Python or harness execution was performed. Runtime `ee36c4c` is integrated as `07c2beb` and passed static review after fixing IR profile nesting and host mapping lease lifetime; no tests/build/fmt/Cargo/Python checks ran, so runtime gates remain pending and no runtime pass is claimed. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34ca-exact-harness-gates-for-s3-20ep-rcx-indexed-absolute-sib-dword-load) и [report](../reports/STAGE_3_S3-34ca_2026-11-02.md) |
| [partial] | S3-34cb | Exact harness gates for S3-20eq RCX-indexed no-base SIB Dword store | Harness source `e16f6fd`, integrated as `3fa9448`, pins exactly `memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20eq_rcx_index_no_base_sib_disp32_dword_store_uses_signed_displacement` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 230 Rust gates (220 acceptance, 10 lifecycle), 248 local steps; independent Sol/high review PASS. No Python tests or other harness execution. Runtime `c384bce` is integrated as `8c50c90` and passed static review after fixing compiler/block/host byte gates and host overflow/backend expectations; no tests/build/fmt/Cargo/Python checks ran, so the exact runtime gates remain pending and no runtime pass is claimed. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cb-exact-harness-gates-for-s3-20eq-rcx-indexed-no-base-sib-dword-store) и [report](../reports/STAGE_3_S3-34cb_2026-11-02.md) |
| [partial] | S3-34cc | Exact harness gates for S3-20er RCX-indexed no-base scaled SIB Dword loads | Harness source `af854bf`, integrated as `0567f37`, pins exactly `memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20er_rcx_index_no_base_sib_scaled_disp32_dword_load_uses_full_width_scaled_index` as `od-jit --lib` / `od-host --lib` acceptance gates. Historical static manifest: 232 Rust gates (222 acceptance, 10 lifecycle), 250 local steps; Sol/high static review PASS. No Python/harness execution. Runtime `0ad1f51` is now integrated as `edf50fe` and passed static review, but no tests/build/fmt/Cargo/Python checks ran; runtime gates remain pending and no runtime pass is claimed. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cc-exact-harness-gates-for-s3-20er-rcx-indexed-no-base-scaled-sib-dword-loads) и [report](../reports/STAGE_3_S3-34cc_2026-11-02.md) |
| [partial] | S3-20es | Exact scaled RCX-indexed no-base SIB disp32 Dword stores | Admit only unprefixed seven-byte forms `89 04 4D/8D/CD disp32` (scales 2/4/8): full-width wrapping RCX×scale plus signed disp32, one contiguous checked low32 RAX width-four write, and `A+7` boundary. Runtime `ca14bac` integrated as `0bf9a46`; Sol/high static review PASS after fixture corrections. No tests/build/fmt/Cargo/Python checks; exact gates pending. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20es-exact-scaled-rcx-indexed-no-base-sib-disp32-dword-stores), [JIT testing reference](../testing/JIT.md#s3-20es-exact-scaled-rcx-indexed-no-base-sib-disp32-dword-stores), и [report](../reports/STAGE_3_S3-20es_2026-11-02.md) |
| [partial] | S3-34cd | Exact harness gates for S3-20es RCX-indexed no-base scaled SIB Dword store | Harness source `beb1c93`, integrated as `a76e732`, pins exactly `memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20es_rcx_index_no_base_sib_scaled_disp32_dword_store_uses_full_width_scaled_index` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 234 Rust gates (224 acceptance, 10 lifecycle), 252 local steps, 101 self-test names; independent Sol/high static review PASS. No Python/harness execution. Runtime `ca14bac` is integrated as `0bf9a46` and passed static review after fixture corrections, but no tests/build/fmt/Cargo/Python checks ran; exact runtime gates remain pending and no runtime pass is claimed. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cd-exact-harness-gates-for-s3-20es-rcx-indexed-no-base-scaled-sib-dword-store) и [report](../reports/STAGE_3_S3-34cd_2026-11-02.md) |
| [partial] | S3-34ce | Exact harness gates for S3-20et addr32 RCX-indexed no-base SIB Dword load | Harness source `2f3198f`, integrated as `4626a9e`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20et_addr32_rcx_index_no_base_sib_disp32_dword_load_uses_wrapped_ecx` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 236 Rust gates (226 acceptance, 10 lifecycle), 254 local steps, 102 self-test names; independent static review PASS. No Python/harness execution. Runtime `8981622` is integrated as `2b04cfd` and passed Sol/high static review after an IR nesting correction, but no tests/build/fmt/Cargo/Python checks ran; exact runtime gates remain pending. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34ce-exact-harness-gates-for-s3-20et-addr32-rcx-indexed-no-base-sib-dword-load) и [report](../reports/STAGE_3_S3-34ce_2026-11-02.md) |
| [partial] | S3-20et | Exact addr32 RCX-indexed no-base SIB disp32 Dword load | Admit only `67 8B 04 0D disp32`: effective address is `zero_extend((low32(RCX)+signed_disp32) mod 2^32)`, one contiguous checked width-four read including 4-GiB crossing, EAX zero-extension, and `A+8` boundary. Runtime `8981622` integrated as `2b04cfd`; Sol/high static review PASS after IR nesting correction. No tests/build/fmt/Cargo/Python checks; exact gates pending. See [JIT contract](../specs/JIT_CONTRACT.md#s3-20et-exact-addr32-rcx-indexed-no-base-sib-disp32-dword-load), [JIT testing reference](../testing/JIT.md#s3-20et-exact-addr32-rcx-indexed-no-base-sib-disp32-dword-load), and [report](../reports/STAGE_3_S3-20et_2026-11-02.md) |
| [partial] | S3-20eu | Exact addr32 RCX-indexed no-base SIB disp32 Dword store | Admit only `67 89 04 0D disp32`, with exactly the `67` addr32 prefix and no others: EA is `zero_extend((low32(RCX)+signext(disp32)) mod 2^32)`, one contiguous checked width-four write of low32 RAX including a 4-GiB crossing, preserve GPR/RFLAGS, and boundary `A+8`. Runtime `a70dedd` integrated as `d56732d`; Sol/high static review PASS after block candidate and host publication byte-gate fixes. No tests/build/fmt/Cargo/Python checks; exact gates pending. See [JIT contract](../specs/JIT_CONTRACT.md#s3-20eu-exact-addr32-rcx-indexed-no-base-sib-disp32-dword-store), [JIT testing reference](../testing/JIT.md#s3-20eu-exact-addr32-rcx-indexed-no-base-sib-disp32-dword-store), and [report](../reports/STAGE_3_S3-20eu_2026-11-02.md) |
| [partial] | S3-34cf | Exact harness gates for S3-20eu addr32 RCX-indexed no-base SIB Dword store | Harness source `2d12a6b`, integrated as `11520e9`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20eu_addr32_rcx_index_no_base_sib_disp32_dword_store_uses_wrapped_ecx` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 238 Rust gates (228 acceptance, 10 lifecycle), 256 local steps, 103 self-test names; independent Sol/high static review PASS. No Python/harness execution. Runtime `a70dedd` is integrated as `d56732d` and passed static review after block candidate and host publication byte-gate fixes, but no tests/build/fmt/Cargo/Python checks ran; exact runtime gates remain pending and no runtime pass is claimed. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cf-exact-harness-gates-for-s3-20eu-addr32-rcx-indexed-no-base-sib-dword-store) и [report](../reports/STAGE_3_S3-34cf_2026-11-02.md) |
| [partial] | S3-20ev | Exact addr32 scaled RCX-indexed no-base SIB disp32 Dword loads | Admit only eight-byte forms with exactly prefix `67` and no others: `67 8B 04 4D/8D/CD disp32` (scales 2/4/8), EA `zero_extend((low32(RCX)*scale+signed disp32) mod 2^32)`, contiguous checked width-four read including 4-GiB crossing, EAX zero-extension, preserve other GPR/RFLAGS, and `A+8`. Runtime `581a5c8` integrated as `a2dc7b8`; Sol/high static review PASS. No tests/build/fmt/Cargo/Python checks; exact gates pending. See [JIT contract](../specs/JIT_CONTRACT.md#s3-20ev-exact-addr32-scaled-rcx-indexed-no-base-sib-disp32-dword-loads), [JIT testing reference](../testing/JIT.md#s3-20ev-exact-addr32-scaled-rcx-indexed-no-base-sib-disp32-dword-loads), and [report](../reports/STAGE_3_S3-20ev_2026-11-02.md) |
| [partial] | S3-20ew | Exact addr32 scaled RCX-indexed no-base SIB disp32 Dword stores | Admit only eight-byte forms with exactly prefix `67` and no others: `67 89 04 4D/8D/CD disp32` (scales 2/4/8), EA `zero_extend((low32(RCX)*scale+signed disp32) mod 2^32)`, contiguous checked width-four low32 RAX store including 4-GiB crossing, preserve GPR/RFLAGS, and `A+8`. Runtime `75346b6` integrated as `af05d53`; Sol/high static review PASS after matcher whitelist and fixture fixes. No tests/build/fmt/Cargo/Python checks; exact gates pending. See [JIT contract](../specs/JIT_CONTRACT.md#s3-20ew-exact-addr32-scaled-rcx-indexed-no-base-sib-disp32-dword-stores), [JIT testing reference](../testing/JIT.md#s3-20ew-exact-addr32-scaled-rcx-indexed-no-base-sib-disp32-dword-stores), and [report](../reports/STAGE_3_S3-20ew_2026-11-02.md) |
| [partial] | S3-20ex | Exact address64 RAX-index/no-base SIB Dword load `8B 04 05 disp32` (`MOV EAX,[RAX+signed disp32]`) through shared typed IR | Только семибайтовый unprefixed singleton: no base, RAX index, scale 1, Qword address and Dword transfer. EA uses original full64 RAX plus sign-extended disp32 with wrapping `u64` arithmetic; one checked contiguous width-four read writes EAX/zero-extends RAX and commits `A+7`, preserving other GPRs/RFLAGS. Runtime `b35b578` plus reviewed coverage/assertion follow-ups `218c6e8`, `913011a`, `0dee0b6`, `a066fdd` integrated as `34011f0`, `abc6506`, `8c0cf1d`, `0df5ada`, `4f2972d`; Sol/high static review PASS. No tests/build/fmt/Cargo/Python checks; exact gates pending. Harness 34ci pins both exact identities; see its report. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-20ex-exact-address64-rax-indexed-no-base-sib-dword-load), [34ci report](../reports/STAGE_3_S3-34ci_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20ex_2026-11-02.md) |
| [partial] | S3-20ey | Exact address64 RAX-index/no-base SIB Dword store `89 04 05 disp32` (`MOV [RAX+signed disp32],EAX`) through shared typed IR | Admit only the seven-byte unprefixed singleton with no base, RAX index, scale 1, Qword address and Dword transfer. EA uses original full64 RAX plus sign-extended disp32 with wrapping `u64`; one checked contiguous width-four store writes low32 original RAX, preserves all GPR/RFLAGS, and commits `A+7`. Runtime `ecc633a` plus follow-up `37c33d1` integrated as `2ce7d6c` and `2a660e4`; Sol/high static review PASS after publisher whitelist, mapping-lifetime and overflow-assertion fixes. No tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34cj pins the JIT/host identities; static harness review passed but no harness execution occurred. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20ey-exact-address64-rax-indexed-no-base-sib-dword-store), [JIT testing reference](../testing/JIT.md#s3-20ey-exact-address64-rax-indexed-no-base-sib-dword-store), [S3-34cj report](../reports/STAGE_3_S3-34cj_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20ey_2026-11-02.md) |
| [partial] | S3-20ez | Exact address64 RAX-index/no-base scaled SIB Dword loads `8B 04 45/85/C5 disp32` (`MOV EAX,[RAX*scale+signed disp32]`) through shared typed IR | Admit only seven-byte unprefixed forms for scales 2/4/8. EA is `(original full64 RAX × scale + sign-extended disp32) mod 2^64`, computed before EAX writeback; one checked contiguous width-four read zero-extends into RAX and commits `A+7`, preserving other GPRs/RFLAGS. Runtime `cda869f` plus host-fixture fixes `a9cc24a` passed Sol/high static review. No tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34ck pins both identities and passed independent static review; no harness run occurred. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20ez-exact-address64-scaled-rax-indexed-no-base-sib-dword-loads), [JIT testing reference](../testing/JIT.md#s3-20ez-exact-address64-scaled-rax-indexed-no-base-sib-dword-load), [S3-34ck report](../reports/STAGE_3_S3-34ck_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20ez_2026-11-02.md) |
| [partial] | S3-34cg | Exact harness gates for S3-20ev addr32 scaled RCX-indexed no-base SIB Dword load | Harness source `7e4f5e8`, integrated as `44e76ed`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20ev_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_uses_wrapped_scaled_ecx` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 240 Rust gates (230 acceptance, 10 lifecycle), 258 local steps, 104 self-test names; independent Sol/high static review PASS. No Python/harness execution. Runtime `581a5c8` is integrated as `a2dc7b8` and passed Sol/high static review, but no tests/build/fmt/Cargo/Python checks ran; exact runtime gates remain pending and no runtime pass is claimed. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cg-exact-harness-gates-for-s3-20ev-addr32-scaled-rcx-indexed-no-base-sib-dword-load) и [report](../reports/STAGE_3_S3-34cg_2026-11-02.md) |
| [partial] | S3-34ch | Exact harness gates for S3-20ew addr32 scaled RCX-indexed no-base SIB Dword store | Harness source `5c916db`, integrated as `29afd24`, pins exactly `memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20ew_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_uses_wrapped_scaled_ecx` as `od-jit --lib` / `od-host --lib` acceptance gates. Static manifest: 242 Rust gates (232 acceptance, 10 lifecycle), 260 local steps, 105 self-test names; independent Sol/high static review PASS. No Python/harness execution. Runtime `75346b6` is integrated as `af05d53` and passed Sol/high static review after matcher whitelist and fixture fixes, but no tests/build/fmt/Cargo/Python checks ran; exact runtime gates remain pending. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34ch-exact-harness-gates-for-s3-20ew-addr32-scaled-rcx-indexed-no-base-sib-dword-store) и [report](../reports/STAGE_3_S3-34ch_2026-11-02.md) |
| [partial] | S3-34ci | Exact harness gates for S3-20ex RAX-indexed no-base SIB Dword load | Harness source `7da38a1` + count fix `27492d7` integrated as `da9d6ed` + `506aa54`, pins exactly `memory_load_v2::tests::ir_dword_rax_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20ex_rax_index_no_base_sib_disp32_dword_load_uses_original_rax`. Static manifest: 244 Rust gates (234 acceptance, 10 lifecycle), 262 local steps, 106 self-test names; independent Sol/high review PASS. No Python/Cargo/harness execution. Runtime S3-20ex is statically reviewed but exact gates remain pending. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34ci-exact-harness-gates-for-s3-20ex-rax-indexed-no-base-sib-dword-load) and [report](../reports/STAGE_3_S3-34ci_2026-11-02.md) |
| [partial] | S3-34cj | Exact harness gates for S3-20ey RAX-indexed no-base SIB Dword store | Harness source `2ebb96c` + filter fix `83f8c72` integrated as `004eaf5` + `c95c511`, pins exactly `memory_load_v2::tests::ir_dword_rax_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20ey_rax_index_no_base_sib_disp32_dword_store_uses_full_rax_address_and_low_dword_data`. Static manifest: 246 Rust gates (236 acceptance, 10 lifecycle), 264 local steps, 107 self-test names; independent Sol/high review PASS. No Python/Cargo/harness execution; runtime exact gates remain pending despite static runtime review PASS. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cj-harness-gates-for-s3-20ey-rax-indexed-dword-store), [harness report](../reports/STAGE_3_S3-34cj_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20ey_2026-11-02.md) |
| [partial] | S3-34ck | Exact harness gates for S3-20ez scaled RAX-indexed no-base SIB Dword load | Harness source `2dc2bc4`, integrated as `1b8ebba`, pins `memory_load_v2::tests::ir_dword_rax_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20ez_rax_index_no_base_sib_scaled_disp32_dword_load_uses_original_scaled_rax`. Static manifest: 248 Rust gates (238 acceptance, 10 lifecycle), 266 local steps, 108 self-test names; independent Sol/high review PASS. No Python/harness execution; runtime exact gates remain pending despite static runtime review PASS. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34ck-harness-gates-for-s3-20ez-scaled-rax-indexed-dword-load), [harness report](../reports/STAGE_3_S3-34ck_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20ez_2026-11-02.md) |
| [partial] | S3-20fa | Exact address64 scaled RAX-indexed no-base SIB Dword store `89 04 45/85/C5 disp32` | Admit only seven-byte unprefixed scales 2/4/8; EA is `(original full64 RAX × scale + signext(disp32)) mod 2^64`, and one contiguous checked width-four write stores low32 of original RAX before committing `A+7`. Runtime `7479a1e` passed Sol/high static review. No tests/build/fmt/Cargo/Python checks; exact gates pending. S3-34cl pins its JIT/host identities and also passed static review; no harness run. ISSUE-045 remains open; Stage 2 excluded/ignored; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fa-exact-address64-scaled-rax-indexed-no-base-sib-dword-store), [JIT testing reference](../testing/JIT.md#s3-20fa-exact-address64-scaled-rax-indexed-no-base-sib-dword-store), [S3-34cl report](../reports/STAGE_3_S3-34cl_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fa_2026-11-02.md) |
| [partial] | S3-34cl | Exact harness gates for S3-20fa scaled RAX-indexed no-base SIB Dword store | Harness source `f349bad`, present at base `7479a1e`, pins `memory_load_v2::tests::ir_dword_rax_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20fa_rax_index_no_base_sib_scaled_disp32_dword_store_uses_scaled_rax_address_and_low_dword_data`. Static manifest: 250 Rust gates (240 acceptance, 10 lifecycle), 268 local steps, 109 self-test names; independent Sol/high review PASS. No Python/Cargo/harness execution; runtime exact gates remain pending despite static runtime review PASS. ISSUE-045 remains open; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cl-harness-gates-for-s3-20fa-scaled-rax-indexed-dword-store) и [report](../reports/STAGE_3_S3-34cl_2026-11-02.md) |
| [partial] | S3-34cm | Exact harness gates for S3-20fb addr32 RAX-indexed no-base SIB Dword load | Harness source `971554d`, integrated as `971554d`, pins `memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20fb_addr32_rax_index_no_base_sib_disp32_dword_load_uses_original_wrapped_eax`. Static manifest: 252 Rust gates (242 acceptance, 10 lifecycle), 270 local steps, 110 self-test names; independent Sol/high review PASS. No Python/Cargo/harness execution. Runtime `bf8dae1` passed separate Sol/high static review; behavior gates remain pending. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cm-harness-gates-for-s3-20fb-addr32-rax-indexed-dword-load) и [report](../reports/STAGE_3_S3-34cm_2026-11-02.md) |
| [partial] | S3-20fb | Exact address32 RAX-indexed no-base SIB Dword load `67 8B 04 05 disp32` | Admit only the eight-byte form with address-size override: EA is `zero_extend_64((low32(original RAX) + signext(disp32)) mod 2^32)`, one contiguous checked width-four read zero-extends EAX into RAX, and successful retirement commits `A+8`. Runtime `bf8dae1` passed Sol/high static review. No behavior tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34cm pins its exact JIT/host identities and passed separate static review; no harness run. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fb-exact-address32-rax-indexed-no-base-sib-dword-load), [JIT testing reference](../testing/JIT.md#s3-20fb-exact-address32-rax-indexed-no-base-sib-dword-load), [S3-34cm report](../reports/STAGE_3_S3-34cm_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fb_2026-11-02.md) |
| [partial] | S3-34cn | Exact harness gates for S3-20fc addr32 RAX-indexed no-base SIB Dword store | Harness source `66ad423`, plus count-assertion fix `f5c353e`, pins `memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20fc_addr32_rax_index_no_base_sib_disp32_dword_store_uses_wrapped_eax_address_and_low_dword_data`. Static manifest: 254 Rust gates (244 acceptance, 10 lifecycle), 272 local steps, 111 self-test names; independent Sol/high review PASS after the count typo fix. No Python/Cargo/harness execution. Runtime `3de12be` passed separate Sol/high static review; exact behavior gates remain pending. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cn-harness-gates-for-s3-20fc-addr32-rax-indexed-dword-store) и [report](../reports/STAGE_3_S3-34cn_2026-11-02.md) |
| [partial] | S3-20fc | Exact address32 RAX-indexed no-base SIB Dword store `67 89 04 05 disp32` | Admit only the eight-byte addr32 form: EA is `zero_extend_64((low32(original RAX) + signext(disp32)) mod 2^32)`; one contiguous checked width-four write stores low32 of original RAX and commits `A+8`. Runtime `3de12be` passed Sol/high static review. No behavior tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34cn pins its JIT/host identities and passed static review after count-assertion fix; no harness run. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fc-exact-address32-rax-indexed-no-base-sib-dword-store), [JIT testing reference](../testing/JIT.md#s3-20fc-exact-address32-rax-indexed-no-base-sib-dword-store), [S3-34cn report](../reports/STAGE_3_S3-34cn_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fc_2026-11-02.md) |
| [partial] | S3-34co | Exact harness gates for S3-20fd addr32 scaled RAX-indexed no-base SIB Dword load | Harness source `4e7619d`, with fail-closed tail restoration `9b1fa57`, pins `memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site` and `tests::s3_20fd_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_uses_original_wrapped_scaled_eax`. Static manifest: 256 Rust gates (246 acceptance, 10 lifecycle), 274 local steps, 112 self-test names; independent Sol/high review PASS after restoring distinct 34cn/34co fail-closed tails. No Python/Cargo/harness execution. Runtime `faaac0f` + lifetime fix `7bc7995` passed separate Sol/high review; exact behavior gates remain pending. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34co-harness-gates-for-s3-20fd-addr32-scaled-rax-indexed-dword-load) и [report](../reports/STAGE_3_S3-34co_2026-11-02.md) |
| [partial] | S3-20fd | Exact address32 scaled RAX-indexed no-base SIB Dword load `67 8B 04 45/85/C5 disp32` | Admit only eight-byte forms at scales 2/4/8; EA is `zero_extend_64((low32(original RAX) × scale + signext(disp32)) mod 2^32)`. One contiguous checked width-four read zero-extends EAX into RAX and commits `A+8`; transfer bytes do not wrap at 4 GiB. Runtime `faaac0f` plus lifetime fix `7bc7995` passed Sol/high static review. No behavior tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34co pins exact JIT/host identities; Sol/high review passed after restoring distinct 34cn/34co fail-closed tails, but harness was not run. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fd-exact-address32-scaled-rax-indexed-no-base-sib-dword-load), [JIT testing reference](../testing/JIT.md#s3-20fd-exact-address32-scaled-rax-indexed-no-base-sib-dword-load), [S3-34co report](../reports/STAGE_3_S3-34co_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fd_2026-11-02.md) |
| [partial] | S3-34cp | Exact harness gates for S3-20fe addr32 scaled RAX-indexed no-base SIB Dword store | Harness source `6976fa4` pins `memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site` and `tests::s3_20fe_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_uses_wrapped_scaled_eax_address_and_low_dword_data`. Static manifest: 258 Rust gates (248 acceptance, 10 lifecycle), 276 local steps, 113 self-test names; independent Sol/high review PASS after syntax/count fixes. No Python/Cargo/harness execution. Runtime `871a350` passed separate Sol/high static review; exact behavior gates remain pending. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cp-harness-gates-for-s3-20fe-addr32-scaled-rax-indexed-dword-store) и [report](../reports/STAGE_3_S3-34cp_2026-11-02.md) |
| [partial] | S3-34cs | Exact harness gates for S3-20fh RAX disp32 Dword load | Harness source `265099e`, forged-argv follow-up `3f85ffd`, and exact-identity correction `60b7086` (integrated as `41f9235`) pin `memory_load_v2::tests::ir_dword_rax_disp32_load_binds_signed_source_bytes_to_its_site` and `tests::s3_20fh_rax_disp32_dword_load_uses_original_full_rax`. Static manifest: 264 Rust gates (254 acceptance, 10 lifecycle), 282 local steps, 116 self-test names; independent Sol/high review PASS after argv and identity fixes. No Python/Cargo/harness execution. Runtime `8f5d9bb` passed separate Sol/high static review after JIT and host coverage fixes; exact behavior gates remain pending. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cs-harness-gates-for-s3-20fh-rax-disp32-dword-load) и [report](../reports/STAGE_3_S3-34cs_2026-11-02.md) |
| [partial] | S3-34ct | Exact harness gates for S3-20fi RAX disp32 Dword store | Harness source `55e1452` pins `memory_load_v2::tests::ir_dword_rax_disp32_store_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20fi_rax_disp32_dword_store_uses_full_rax_address_and_low_dword_data`. Static manifest: 266 Rust gates (256 acceptance, 10 lifecycle), 284 local steps, 117 self-test names; independent Sol/high review PASS. No Python/Cargo/harness execution. Runtime `2c54a0a` plus host/JIT coverage `4098ac1` passed separate Sol/high review; exact behavior gates remain pending. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34ct-harness-gates-for-s3-20fi-rax-disp32-dword-store) и [report](../reports/STAGE_3_S3-34ct_2026-11-02.md) |
| [partial] | S3-34cq | Exact harness gates for S3-20ff RAX disp8 Dword load | Harness source `ee97e4f` pins `memory_load_v2::tests::ir_dword_rax_disp8_load_binds_all_signed_source_bytes_to_its_site` and `tests::s3_20ff_rax_disp8_dword_load_uses_original_rax`. Static manifest: 260 Rust gates (250 acceptance, 10 lifecycle), 278 local steps, 114 self-test names; independent Sol review PASS. No Python/Cargo/harness execution. Runtime `45aca5c` passed separate Sol review; exact behavior gates remain pending. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cq-harness-gates-for-s3-20ff-rax-disp8-dword-load) и [report](../reports/STAGE_3_S3-34cq_2026-11-02.md) |
| [partial] | S3-20ff | Exact address64 RAX-base Dword load with signed disp8 `8B 40 ib` | Admit only the three-byte unprefixed form. EA uses original full-width RAX plus sign-extended disp8 with wrapping u64 semantics; one contiguous checked width-four read zero-extends EAX to RAX and commits `A+3`. Runtime `45aca5c` passed independent Sol review. No behavior tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34cq pins its JIT/host identities and passed separate review; harness not run. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20ff-exact-address64-rax-base-dword-load-with-signed-disp8), [JIT testing reference](../testing/JIT.md#s3-20ff-exact-address64-rax-base-dword-load-with-signed-disp8), [S3-34cq report](../reports/STAGE_3_S3-34cq_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20ff_2026-11-02.md) |
| [partial] | S3-20fe | Exact address32 scaled RAX-indexed no-base SIB Dword store `67 89 04 45/85/C5 disp32` | Admit only eight-byte forms at scales 2/4/8; EA is `zero_extend_64((low32(original RAX) × scale + signext(disp32)) mod 2^32)`. One contiguous checked width-four write stores low32 of original RAX and commits `A+8`; transfer bytes do not wrap at 4 GiB. Runtime `871a350` passed Sol/high static review after two host-fixture fixes. No behavior tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34cp pins exact JIT/host identities and passed static review after syntax/count fixes; no harness run. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fe-exact-address32-scaled-rax-indexed-no-base-sib-dword-store), [JIT testing reference](../testing/JIT.md#s3-20fe-exact-address32-scaled-rax-indexed-no-base-sib-dword-store), [S3-34cp report](../reports/STAGE_3_S3-34cp_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fe_2026-11-02.md) |
| [partial] | S3-20fi | Exact address64 RAX-base Dword store `89 80 disp32` | Admit only the six-byte unprefixed form: effective address is full-width original RAX plus signed disp32 with wrapping u64 semantics; one contiguous checked width-four write stores low32 original RAX and commits `A+6`. Runtime `2c54a0a` and host/JIT coverage `4098ac1` passed Sol/high static review. No behavior tests/build/fmt/Cargo/Python checks; exact gates pending. Harness S3-34ct pins exact JIT/host identities and passed separate Sol/high review; no harness execution. ISSUE-045 remains open; Stage 2 ignored/excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fi-exact-address64-rax-base-dword-store-with-signed-disp32), [JIT testing reference](../testing/JIT.md#s3-20fi-exact-address64-rax-base-dword-store-with-signed-disp32), [S3-34ct report](../reports/STAGE_3_S3-34ct_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fi_2026-11-02.md) |
| [partial] | S3-34cu | Exact harness gates for S3-20fj address32 alias Dword load | Harness pins `memory_load_v2::tests::ir_dword_addr32_alias_load_binds_all_source_bytes_to_its_site` and `tests::s3_20fj_addr32_alias_dword_load_uses_original_eax` as exact `od-jit --lib` / `od-host --lib` local-acceptance gates; required fail-closed self-test is `test_s3_34cu_addr32_alias_dword_load_gates_are_pinned_and_fail_closed`. Independent Sol review PASS. No behavior tests/build/fmt/Cargo/Python/Mini/M1 ran; exact gates pending. ISSUE-045 preserved; Stage 2 excluded; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cu-harness-gates-for-s3-20fj-address32-alias-dword-load) and [report](../reports/STAGE_3_S3-34cu_2026-11-02.md) |
| [partial] | S3-34cv | Exact harness gates for S3-20fk address32 alias Dword store | Harness pins `memory_load_v2::tests::ir_dword_addr32_alias_store_binds_all_source_bytes_to_its_site` and `tests::s3_20fk_addr32_alias_dword_store_uses_original_eax_address_and_payload` with fail-closed exact identity/argv/evidence self-test. Static manifest: 270 Rust gates (260 acceptance, 10 lifecycle), 288 local steps, 119 self-test names. Independent Sol/high review PASS; no Python/Cargo/harness execution and runtime gate execution remains pending. Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34cv-harness-gates-for-s3-20fk-address32-alias-dword-store) and [report](../reports/STAGE_3_S3-34cv_2026-11-02.md) |
| [partial] | S3-20fj | Exact address32 RAX-alias Dword load `67 8B 00` | Admit only the three-byte singleton; effective address is zero-extended low32 of pre-instruction RAX, which is also the load destination. One checked four-byte read loads/zero-extends EAX and commits `A+3`. Runtime `b1aa25d` passed Sol/high static review after host fixture/retry fixes. No behavior tests/build/fmt/Cargo/Python checks; exact gates pending. S3-34cu pins its exact JIT/host identities and passed separate Sol review; no harness execution. ISSUE-045 unchanged; Stage 2 excluded; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fj-exact-address32-rax-alias-dword-load), [JIT testing reference](../testing/JIT.md#s3-20fj-exact-address32-rax-alias-dword-load), [harness report](../reports/STAGE_3_S3-34cu_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fj_2026-11-02.md) |
| [partial] | S3-20fk | Exact address32 RAX-alias Dword store `67 89 00` | Admit only the three-byte singleton; the effective address is zero-extended low32 of original RAX and the payload is that same original value’s low32. One checked contiguous four-byte write preserves GPRs/RFLAGS and commits `A+3`, including stores crossing 4 GiB without byte-wise wrap. Runtime commits `089560b`/`afa89ba` passed independent Sol/high static review; full od-jit suite passed 397/397 and the exact host regression passed after correcting stale fixture expectations (no runtime changes). S3-34cv pins exact JIT/host identities and passed static review, but its harness execution remains pending. ISSUE-045 remains open; Mini/M1 deferred; Stage 3 partial. См. [JIT contract](../specs/JIT_CONTRACT.md#s3-20fk-exact-address32-rax-alias-dword-store), [JIT testing reference](../testing/JIT.md#s3-20fk-exact-address32-rax-alias-dword-store), [S3-34cv report](../reports/STAGE_3_S3-34cv_2026-11-02.md), and [runtime report](../reports/STAGE_3_S3-20fk_2026-11-02.md) |
| [partial] | S3-20fz | Address32 RCX-base Dword load signed disp32 `67 8B 81 id` | Exact seven-byte load; EA is `zero_extend((low32(RCX) + signed_i32_disp32) mod 2^32)`, one contiguous checked four-byte read, zero-extension to RAX, preserved RCX/GPRs/RFLAGS, and `A+7`. Runtime commits `ac69bcd`/`baba8f8` are integrated and passed independent static review. Behavior/build/fmt/Cargo/Python checks remain unrun; see bounded report. |
| [partial] | S3-34dk | Exact harness gates for S3-20fz | Harness commit `2de1cab` pins the exact JIT/host identities and fail-closed self-test; independent static review passed. Manifest totals are pinned at 300 gates (290 acceptance + 10 lifecycle), 318 local steps, 134 self-test names; execution remains pending. |
| [partial] | S3-20ga | Address32 RCX-base Dword store signed disp32 `67 89 81 id` | Seven-byte exact store; starting EA is `zero_extend((low32(RCX) + signed_i32_disp32) mod 2^32)`, one checked contiguous write4 of low32 RAX, preserved GPR/RFLAGS and `A+7`. Runtime `b7b361b` integrated and passed independent Sol/high review. No tests/build/fmt/Cargo/Python checks run; see bounded report. |
| [partial] | S3-34dl | Exact harness gates for S3-20ga | Harness `c2a3e8f` pins the JIT source/site, host checked-store and fail-closed self-test identities; independent Sol/high review passed. Totals 302 gates (292 acceptance + 10 lifecycle), 320 local steps, 135 self-test names are pinned/reviewed but not executed. |
| [partial] | S3-34dm | Fixed-seed differential corpus for S3-20eg `67 89 04 24` | Corpus `c7bb9c7` and harness `2fb1265` are integrated; S3-34.1/.3/.5 passed independent static Sol/high review. Exact local corpus gate passed 1/1 and the fail-closed Python harness self-test exited 0. S3-34dm pinned totals: 303 gates (293 acceptance + 10 lifecycle), 321 local steps, 136 self-test names; current manifest after S3-34dr: 324 gates (306 acceptance + 18 lifecycle), 342 steps, 149 self-tests, 18 sources. These counts do not imply a full harness run. S3-34.2 runtime/replay acceptance remains pending. ISSUE-045 was resolved separately. Physical x86/Mini, full Stage 3, owner/MMU, and performance acceptance remain open; M1 is deferred. См. [JIT testing reference](../testing/JIT.md#s3-34dm-fixed-seed-differential-corpus-for-s3-20eg) and [report](../reports/STAGE_3_S3-34dm_2026-11-02.md) |
| [partial] | S3-34dn | Create-only mismatch artifact and validated replay for S3-34dm | Runtime commits `f858b7a`/`10447f0`/`ee2af23` and harness commits `89c5e28`/`13b7790` are integrated and passed independent static Sol/high review. Rust artifact-protection self-check and pinned Python self-test passed; they use synthetic observations and do not prove JIT/native parity or real-artifact replay. Historical totals for this slice: 304 gates (294 acceptance + 10 lifecycle), 322 local steps, 137 self-test names; current manifest after S3-34dr is 324 gates (306 acceptance + 18 lifecycle), 342 local steps, 149 self-test names. S3-34.2 runtime/replay acceptance remains pending. ISSUE-045 is resolved for the separate S3-20eg retry-assertion host gate; ISSUE-046 is resolved for this synthetic artifact-protection self-check only. См. [JIT testing reference](../testing/JIT.md#s3-34dn-create-only-mismatch-artifact-and-validated-replay-for-s3-34dm) and [report](../reports/STAGE_3_S3-34dn_2026-11-03.md) |
| [partial] | S3-34do | Bounded value-only reducer for one S3-34dn artifact | Runtime commits `5ae5e47`/`0e64b07` and harness commit `2c24eec` are integrated and passed independent static Sol/high review. Exact synthetic reducer self-check passed 1/1 and pinned Python self-test exited 0; no real S3-34dn artifact was replayed or reduced, and no JIT parity is established. Historical totals: 305 gates (295 acceptance + 10 lifecycle), 323 steps, 138 self-test names; current manifest after S3-34dr is 324 gates (306 acceptance + 18 lifecycle), 342 steps, 149 self-test names. S3-34.2 execution acceptance remains pending. ISSUE-045 is resolved for the separate S3-20eg retry-assertion host gate; ISSUE-046 is resolved for the S3-34dn artifact-protection self-check only. Mini unavailable, M1 deferred; Stage 3 partial. См. [JIT testing reference](../testing/JIT.md#s3-34do-bounded-value-only-reducer-for-one-s3-34dn-artifact) and [report](../reports/STAGE_3_S3-34do_2026-11-03.md) |
| [partial] | S3-34dp | Exact local harness gates for S3-38e session ownership | Four exact `od-host --lib` lifecycle identities and the fail-closed harness self-check are pinned, including the `session.rs` source fingerprint; all four Rust gates passed 1/1 and the pinned self-test exited 0. Current totals after S3-34dr: 324 gates (306 acceptance + 18 lifecycle), 342 local steps, 149 self-tests, 18 sources. Full Stage 3 harness and physical x86/Mini acceptance remain open; M1 deferred. См. [JIT testing reference](../testing/JIT.md#s3-38e-bounded-local-rust-session-owner) and [report](../reports/STAGE_3_S3-34dp_2026-11-03.md) |
| [partial] | S3-34dq | Exact Store→Store fixed-seed corpus, replay/reducer coverage, and gates | Adds 1,200 unique exact `48 89 03 48 89 02` inputs, strict profile `s3-26f-two-site-qword-store-store-v1`, and second-write fault/continuation no-replay evidence. Three exact od-host corpus gates, the od-testkit generator gate, and S3-26f fail-closed Python self-test pass 1/1. Current harness totals: 324 Rust gates (306 acceptance + 18 lifecycle), 342 local steps, 149 self-test names, 18 source pins. Full harness unittest timed out after 180s without final result; not classified as failure. Mini not run; M1 deferred. Physical-x86, owner/MMU and performance acceptance remain open; ISSUE-045 stays open. См. [implementation report](../reports/STAGE_3_S3-34dq_2026-11-03.md) |
| [partial] | S3-34dr | Exact local harness gates for retained Load→Load S3-18f | Pins the two JIT profile/entry-boundary gates and one od-host retained load-pair regression with exact local acceptance identities; adds a fail-closed source/gate/evidence self-test. Totals 324 Rust gates (306 acceptance + 18 lifecycle), 342 local steps, 149 self-test names, and 18 Rust source pins. Sol/high review PASS; the new self-test and all three exact Rust gates passed 1/1. Full unittest run exceeded 180s and was killed without a final result (not classified as failure). Mini has not run; M1 remains deferred. См. [implementation report](../reports/STAGE_3_S3-34dr_2026-11-03.md) and [JIT testing reference](../testing/JIT.md#s3-34dr-exact-harness-gates-for-the-s3-18f-loadload-profile) |
| [partial] | S3-34ds | Generated mixed S3-55 register corpus | 1,200 unique inputs from three seeds run twice in the interpreter and on the real ARM64 arena for each flags mode. The integration test is now required by the local selector. The selected Rust gate and all 158 harness self-tests passed on local ARM64. Stage-wide and physical gates remain open. [Report](../reports/STAGE_3_S3-34ds_2026-11-19.md) |
| [partial] | S3-34dt | Reduce a generated NativeRegisterV1 cache-trace GPR mismatch | Exact ordered-prefix replay and register-only reduction preserve the mismatch class and record all three backend observations. Four signed ARM64 gates passed, including a controlled observation mismatch and ordinary rejection of an unreproduced artifact. Broader S3-34.2 remains open. [Report](../reports/STAGE_3_S3-34dt_2026-11-19.md) |
| [partial] | S3-35 | Измерить холодное исполнение | S3-01, S3-34 |
| [partial] | S3-36 | Измерить прогретое исполнение | S3-35, локальные замеры S3-36a–g |
| [ ] | S3-37 | Cache churn и ресурсы | S3-25, S3-34 |
| [ ] | S3-38 | Интеграция VMHost и ABI | S3-28, S3-34 |
| [partial] | S3-38a | Register-only S0-09 V0 C ABI through run-local eager block dispatcher | Immutable fragment, per-call arena/cache, C ABI lifecycle and local VMHost process regressions; full cache/request ownership and all S3-38 gates remain open |
| [partial] | S3-38c | Sequential cross-thread V0 handle lifecycle | One idle handle called sequentially from two host threads, second-handle interleaving, pre-run STOP and per-call mapping release; persistent cache and full S3-38 remain open |
| [partial] | S3-38e | Bounded opt-in local Rust S0-09 session owner | Retains copied code/admission, bounded cache and one executable-memory lease across synchronous calls; no VMHost IPC RUN/session or production MMU integration. Four exact macOS/ARM64 local lifecycle gates passed 1/1 each; the pinned harness self-test exited 0. Full Stage 3 harness and physical x86/Mini acceptance remain open; M1 deferred. См. [implementation report](../reports/STAGE_3_S3-38e_session_owner_2026-11-03.md) and [harness report](../reports/STAGE_3_S3-34dp_2026-11-03.md) |
| [partial] | S3-38f | Rust C ABI proxy owning one S0-09 Session on a dedicated OS thread | Three exact macOS/ARM64 Rust lifecycle gates and the pinned harness self-test passed. The signed C client verified linking/layout, normal delivery, injected reader loss and pipe creation failure with `EMFILE` locally and on M1 Pro. Broader race ordering, Mac mini, physical x86 and full S3-38 acceptance remain open. См. [implementation report](../reports/STAGE_3_S3-38f_session_proxy_2026-11-03.md) |
| [partial] | S3-38g | VMHost control V1 repeated same-code RUN transport | Adds explicit opt-in V1 selection, session ABI adapter, event-driven socket/completion loop, incremental bounded frame staging and a 16,416-byte output queue; V0 remains the default path. Full Sol/high static review passed. No build, test or runtime execution was performed at the S3-38g integration revision; the later focused S3-38h process smoke passed. Broader V1 acceptance gates remain open. См. [implementation report](../reports/STAGE_3_S3-38g_vmhost_v1_transport_2026-11-03.md) |
| [partial] | S3-38h | Process-level V1 same-code repeated RUN smoke and exact process-harness gate | Release od-host build, Swift VMHost compile, ad-hoc runtime signature/strict verification and `allow-jit` entitlement verification passed. The exact process smoke passed with two decoder-supported ADD RAX,RBX instructions, fresh state/budgets, increasing IDs, complete RESULT payloads, ACK/RESULT ordering and idle EOF teardown; manifest self-test passed. Current harness totals are 324 gates (306 acceptance + 18 lifecycle), 342 steps, 149 self-tests, 18 source pins. Full 39-step VMHost suite, C-client layout/link, pipe-failure, active STOP/EOF and framing acceptance remain open. Mini SSH timed out before remote tests; M1 deferred; Stage 3 partial. См. [implementation report](../reports/STAGE_3_S3-38h_vmhost_v1_process_2026-11-03.md) |
| [partial] | S3-38i | Process-level STOP isolation and EOF during active V1 RUN | Exact bounded smoke passed: STOP_ACK→RESULT ordering, stopped result, fresh same-code invocation, and clean teardown after active-run EOF. Descriptor/lease cleanup before exit, broader process suite, Mini/physical-x86 and full Stage 3 acceptance remain open. См. [implementation report](../reports/STAGE_3_S3-38i_vmhost_v1_stop_eof_2026-11-03.md) |
| [pass] | S3-38j | Process-level fragmented/coalesced V1 frames and bounds | Local signed suite passed 46/46. Covers maximum fragmented RUN, coalesced BUSY, truncated and oversized frames, output backpressure with active completion, and batch closure after injected create/begin INVALID_STATE. Decoder and response staging limits were audited and traced. См. [implementation report](../reports/STAGE_3_S3-38j_vmhost_v1_frames_2026-11-03.md) |
| [pass] | S3-38k | Process-level immutable-code mismatch and RUN-ID high-watermark | Focused ARM64 process smoke and both manifest checks pass; Mini SSH timed out before remote commands. Full process-suite and broader V1 acceptance remain open. См. [implementation report](../reports/STAGE_3_S3-38k_vmhost_v1_code_mismatch_2026-11-03.md) |
| [pass] | S3-38l | Process-level active STOP, rejected RUN/STOP IDs and RUN-ID high-watermark | Focused ARM64 process smoke and fail-closed manifest test pass; Mini SSH timed out before remote commands. Full process-suite and broader V1 acceptance remain open. См. [implementation report](../reports/STAGE_3_S3-38l_vmhost_v1_run_stop_id_2026-11-03.md) |
| [pass] | S3-38m | Process-level RUN-ID exhaustion and fresh-helper reset | Focused ARM64 process smoke and fail-closed manifest test pass using decoder-supported ADD; physical Mini and full process-suite acceptance remain open. См. [implementation report](../reports/STAGE_3_S3-38m_vmhost_v1_id_exhaustion_2026-11-03.md) |
| [pass] | S3-38n | V1 process isolation after a helper SIGSEGV | Focused two-helper smoke passed: both admitted `JNZ -2` RUNs were active, SIGSEGV terminated only helper A, helper B returned exact-ID STOP_ACK before its full unchanged-state RESULT, then completed same-code fresh-state RUN and idle EOF. Exact process gate and fail-closed manifest self-check passed against a locally rebuilt and signed VMHost. The full process suite, physical Mini, and full Stage 3 remain open. См. [implementation report](../reports/STAGE_3_S3-38n_vmhost_v1_isolation_2026-11-03.md) |
| [x] | S3-38o | Production host loop одного vCPU для `PagedFetchedLoadSession` — принято с ограничениями (gate-ревью Claude Opus, high, 2026-11-18) (внешний STOP, протокол STOP-байта S6-40 в однопоточной форме, общий бюджет, общий интерпретатор для не-JIT форм) | S3-25 (S3-25c), S3-28 (S3-28c), S4-15, S4-16; не зависит от S3-34 и S3-38f–n; блокер S6-42; закрытие не закрывает S3-38. Оценка 2–3 дня-агента |
| [x] | S3-39 | Bounded macOS/ARM smoke passed on M1 Pro; see [report](../reports/STAGE_3_S3-39_M1PRO_2026-10-31.md) | S3-38a |
| [ ] | S3-40 | Зафиксировать решения по данным | S3-35–S3-39 |
| [partial] | S3-40j | Profile-specific offline audit of saved S3-31c reports | Recomputes both paired ratios/ranges and validates profile, workload, schedule, and preflight schema; does not complete S3-40 or Stage 3 |
| [ ] | S3-41 | Документация и приёмка этапа | Все обязательные задачи |

Первый вертикальный путь: `00 → 02 → 03 → 04 → 05`, затем `06 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 14`. Baseline S3-01 выполняется до выбора оптимизаций. Тесты добавляются в каждой задаче, S3-34 объединяет их и проверяет сам стенд.

#### S3-28a. Cooperative event yield на границах register dispatcher

Добавить invocation-local optional atomic event request в host `DispatchControl` и отдельный `DispatchExit::EventPending`; не использовать runtime exit tag и не менять `RuntimeContextV1`/generated ABI. Dispatcher проверяет запрос после stop-RIP → STOP → CodeEnd → budget guards и до следующего lookup/compile/native entry. В `run_with_nop_fallback` тот же порядок действует перед каждым interpreter NOP step. Запрос, выставленный во время native-вызова, становится виден после возврата текущего bounded блока; bare-NOP fallback ограничен одним interpreter step между проверками. Yield оставляет guest state, budget и retirement как их зафиксировал завершённый блок/шаг. Event flag не очищается автоматически: host очищает его и продолжает с фактического RIP.

Приёмка: event до входа сохраняет CPU state/budget/retirement и не вызывает compile, publication или execution, включая NOP fallback; event из executor hook после bounded блока фиксирует ровно один блок и не запускает следующий; очистка и resume совпадают с uninterrupted interpreter без replay prefix/Jcc. Покрыть single-cache hit/miss, multi-cache capacity 1/multi, native→NOP→native, simultaneous guard priority, terminal runtime exits, backend errors и malformed accounting. Отдельный published-arena host test проверяет yield перед resident native entry. Guest-work bound формулируется только как один текущий bounded native block или один bare NOP step; wall-time latency/performance claims и Mini проверки отложены до конца Stage 3. Stage 2, owner/MMU и `od-machine` в этом срезе не меняются. См. [S3-28a report](../reports/STAGE_3_S3-28a_2026-10-29.md).

#### S3-28b. Cooperative event yield на bounded checked-memory dispatcher

Добавить controlled-варианты `run` и `run_with_nop_fallback` с invocation-local optional atomic event request; прежние публичные вызовы остаются event-disabled wrappers. Отдельный `BoundedMemoryExit::EventPending` не меняет V2/V3 ABI, generated code или helper semantics. Dispatcher соблюдает stop-RIP → STOP → fixture boundary → budget → event до compile/executor и перед каждым новым artifact/NOP. Published host executor повторяет stop-RIP → STOP → budget → event после publication и перед native entry; fixture boundary уже проверена dispatcher'ом до compile, а state не меняется до входа. Mapping освобождается при pre-entry exit.

Event, поднятый memory callback, опрашивается только после возврата bounded artifact. Если нет более приоритетного terminal exit, оба helper-вызова пары завершаются до sampling, поэтому такой yield может зафиксировать до двух retired guest instructions. STOP/fault/backend error сохраняют существующую семантику helper boundary и не опрашиваются/маскируются event. Сначала коммитятся valid report/accounting и фактический RIP; `GuestFault`, `CodeInvalidated`, STOP и backend errors сохраняют первичность, malformed reports остаются ошибкой accounting. Event остаётся установленным; caller очищает его и продолжает с actual RIP без replay. Acceptance покрывает публикацию без native entry, singleton load/store, pair → NOP → pair, second-site fault/backend failure, callback STOP+event, guard priority и malformed executor reports. Mini/performance отложены до конца Stage 3; Stage 2, owner/MMU и `od-machine` вне scope.

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

Повтор от 18 ноября 2026 года подтвердил локальный baseline Stage 1 и
совпадение результата на общем register-only входе у interpreter, legacy
fragment, block JIT и dispatcher. Обновлена [методика измерений](../testing/BENCHMARKS.md#jit-этапа-3-workload-и-уровни-прогрева).
Физический x86 preflight на совпадающем fingerprint достиг лимита 1 800 с.
В следующем завершившемся preflight два теста `od-runner` остановились на
отсутствующих файлах отчётов в изолированном checkout. Перед Cargo oracle
теперь сверяет отдельный отпечаток x86-фикстур и трёх отчётов `linux_gate`.
Проверка неполного снимка подтверждена локально. Физические проверки этапа 3
отложены по решению владельца. S3-01 остаётся открытой.
См. [baseline report](../reports/STAGE_3_BASELINE_2026-10-26.md#повтор-на-исходниках-3476780-18-ноября-2026)
и [ISSUE-519](../KNOWN_ISSUES.md#issue-519).

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

**Фактически сделано:** `od-host::PublishedCode::publish` строит байты в private `MAP_JIT`, RAII guard возвращает write protection до cache sync, `sys_icache_invalidate` происходит до выдачи owner. Owner удерживает один mapping и освобождает его в Drop. Проверены отказ второй публикации и повторное использование slot после Drop; C ABI переводит конкурентный host resource отказ в существующий `STATUS_BUSY`, layout/signatures C ABI не менялись. `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `cargo test -p od-host --offline` (8/8), `git diff --check`; layout check прошёл в основном checkout. Независимое review clean после исправления ссылки на ISSUE-038.

Apple SDK 27.0 объявляет `pthread_jit_write_protect_np` доступным с macOS 11.0; явный deployment target проекта не найден. В Hardened Runtime с allow-jit допускается одна MAP_JIT region на процесс, поэтому `od-host` допускает один standalone mapping или одну shared arena. Указанная write protection действует на текущий thread и не является process-wide synchronization с посторонними JIT users; `od-host` не пишет standalone опубликованный code повторно. Подробная граница — [ISSUE-038](../KNOWN_ISSUES.md#issue-038).

Ad-hoc подписанный Hardened Runtime VMHost smoke с allow-jit entitlement и JIT execution через launcher прошёл на macOS 26.7.1 ARM64 / Xcode SDK 27.0 вне debugger. Это локальная проверка текущей машины, не Developer ID signing, notarization, packaged-app acceptance или подтверждение другой версии macOS; ISSUE-004 остаётся открытой. S3-18b добавляет host-only bounded slots внутри одной region, но od-jit multi-entry cache integration и performance-based capacity choice остаются открытыми. Не использовать per-block mmap как multi-block implementation.

### S3-08. Реализовать generated entry и helper calls

1. Зафиксировать prologue/epilogue, stack alignment, callee-saved/platform registers и representation arguments/results.
2. Создать один реальный helper call с объявленными clobbers; проверить state до/после, nested host call и error return.
3. Исключить unwind через generated code и aliasing живых Rust references с generated state access. Host failures не преобразовывать в guest exceptions.

S3-08 выполнена: `runtime` объявляет все `RuntimeContextV1` offsets через `offset_of!`, `BlockEntry`/helper AAPCS64 clobber contract и отдельный raw helper-failure sentinel. `od-host::decode_block_entry_result` отвергает sentinel как host contract failure до guest-exit interpretation. Native ARM64 test-only typed-Encoder probe создаёт frame record через x29/x30, сохраняет x19-x29, вызывает nested Rust helper по ABI `(w0,x1,x2,x3,x4)`, проверяет exact arguments, volatile x1-x17 clobbers, SIMD D8-D15 canaries, status 0/1/2/unknown и failure output staging без guest-state write/retirement. Он не добавляет x86 helper semantics или общий block entry compiler.

`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `cargo test -p od-host --offline` прошли на Apple M3 Max/macOS 26.7.1 (11 tests); generated-entry success/error probes дополнительно прошли в ad-hoc подписанном Hardened Runtime test process с allow-jit entitlement. `git diff --check`, independent review clean и `tools/check_layout.py` прошли. C ABI не менялся. S3-13 ещё должен соединить этот contract с IR lowering; raw generic entry invocation остаётся unsafe.

### S3-09. Реализовать простой allocator baseline

1. Для первого набора temporaries выбрать простой проверяемый mapping/spill strategy с ограниченным scratch register set.
2. Реализовать stack slots и загрузку/сохранение values вокруг helper calls; запрещённые host registers исключить явно.
3. Добавить проверки конфликтов live values и spill slot bounds. Считать spill/reload/code size для будущего сравнения.

S3-09 выполнена: реализован первый baseline в `od-jit::allocator`: fixed homes x9..x12 в порядке определения, budget 0..=4, отдельные W64 spill slots, explicit helper boundaries/liveness/capture, transfer emission и checked frame. Unit tests и native helper-clobber probe добавлены; результат остаётся baseline cost estimate, не benchmark и не оптимизация. Независимое review clean; workspace check, форматирование, `od-jit` 46/46 tests, `od-host` 12/12 tests, diff-check и Graphify update прошли. Ограниченное покрытие комбинаций liveness зарегистрировано в [ISSUE-039](../KNOWN_ISSUES.md#issue-039) и расширяется при S3-10. Production consumption относится к S3-10; allocator не добавляет guest semantics.

### S3-10. Выполнить первый integer lowering

1. Добавить native lowering constants, MOV и арифметического результата ADD/SUB/CMP первого профиля.
2. Правильно обрабатывать guest widths и host zero/sign extension; CMP не записывает arithmetic result в guest destination.
3. Проверить chains temporaries, spills и входные registers с крайними bit patterns.

S3-10 выполнена: `od-jit::emitter::emit_data_effects` потребляет verifier-checked decoder-derived `Program` и связанную с ним `Allocation`; native od-host test проходит полный путь `CodeView → BasicBlock → lowering → allocation → emitter → publication → execution` для MOV imm64 и ADD/SUB/CMP reg64. Проверены register budgets 0..=4, spills, aliasing/equal operands, крайние bit patterns, многословная immediate и неравные SUB/CMP operands с borrow/wrap. `CMP` производит только наблюдаемый scratch result и не записывает гостевой GPR. Arithmetic flags остаются явно неполными; emitter не обновляет canonical `LongModeState`, RIP, budget или retirement и возвращает host-only uncommitted sentinel. Ветка/Jcc и неподдержанные exits отвергаются. Это промежуточные production bytes и staging probe, а не готовая guest-инструкция: S3-11 должен материализовать flags и определить точку архитектурного commit перед успешным guest exit. Проверки: `cargo fmt --all -- --check`, workspace all-target check, `od-jit` 50/50 tests, `od-host` 13/13 tests, `git diff --check`; integration test прошёл вне Cargo invocation в подписанном Hardened Runtime процессe с allow-jit entitlement; independent review clean, layout check успешен. Coverage ограничения allocator указаны в [ISSUE-039](../KNOWN_ISSUES.md#issue-039).

### S3-11. Реализовать eager flags baseline

1. Материализовать определённые CF/PF/AF/ZF/SF/OF для текущих форм, сохраняя требуемые неизменяемые bits.
2. Не отождествлять host NZCV с полным x86 RFLAGS. Jcc читает guest condition, а helper clobber не уничтожает pending значение.
3. Сверить carry/borrow, signed overflow, auxiliary carry и parity по независимым byte fixtures.

S3-11 выполнена: выделен общий `od-isa::integer_flags` helper для W64 ADD/SUB/CMP; тонкий `od-jit` ABI adapter читает output только при status Success, а data-effect emitter оставляет flags/выбранный RIP в uncommitted staging. Native tests покрывают оба исхода JZ/JNZ, MOV flag preservation, CMP без GPR write, helper failure без текущего guest commit и helper-clobber allocator homes. Literal vectors и interpreter cross-checks проверяют шесть определённых flags и сохранение остальных RFLAGS bits. Independent review clean; workspace check, fmt, `od-jit` 51/51, `od-host` 16/16, targeted `integer_flags` 3/3, diff-check и layout check прошли. Полный `od-isa` suite по-прежнему имеет пять несвязанных x87/decoder failures, зарегистрированных в [ISSUE-040](../KNOWN_ISSUES.md#issue-040). Архитектурный commit и recovery остаются в S3-12.

### S3-12. Добавить state points и PC maps

1. Для каждой guest instruction и каждого предусмотренного exit записать guest RIP, host code range, commit state и способ материализации live values.
2. Восстановить точное состояние при helper/fallback/normal exits; не записывать next RIP до проверки faulting операции.
3. Проверить maps после relocation и связывание metadata с lifetime executable block. Padding/prologue/epilogue не выдавать за отдельные guest instructions.

S3-12 implementation добавляет production runtime emitter для текущего W64
register-only среза, commit markers, checked half-open PC ranges и отдельные
committed guest-boundary points. `PublishedCode::publish_block` держит карту с
mapping lifetime; безопасный host wrapper принимает только production-helper
provenance. Native tests проверяют normal exits, внутренние stop/budget guards,
JZ/JNZ selection и helper failure после committed prefix. Canonical state
обновляется лишь после успешных effects; helper failure оставляет текущую
инструкцию на её guest RIP. Общие branch-return/epilogue ranges указывают на
canonical RIP, тогда как site-specific failure stubs хранят фиксированный RIP;
reversed ranges отвергаются валидацией карты. Независимый review прошёл без
оставшихся замечаний; workspace/package checks и primary-checkout layout check
успешны.

Ограничение: ranges описывают emitted host bytes и planned exits, а не дают
универсальное восстановление для произвольного host PC, crash, panic, ABI
violation или asynchronous snapshot. Memory/fault replay отсутствует. Нативный
test evidence относится к production ARM64 block path на настроенном allow-jit
host; физическое сравнение с x86 относится к S3-14.

### S3-13. Подключить один блок к CLI

**Файлы:** существующий od-runner, od-host/od-jit adapters, testkit result path.

1. Выполнить compile → publish → execute → snapshot для первого профиля с теми же входами и compare scope, что у interpreter.
2. Сохранить старый fragment entry/ABI и fixtures. Новый путь должен быть различим в metadata результата, без подмены исторических evidence.
3. Проверить completed, unsupported, allocation/compile error и malformed result paths.

Текущая реализация добавляет opt-in `--fixture CASE_V1.json --engine
jit-block --json` с отдельным `jit_ir_block` backend tag. `od-jit` adapter
сохраняет `NoNativePrefix`, строит allocation/runtime emitter для одного
bounded block и отклоняет generated code свыше 4096 bytes до публикации. Runner
сохраняет старый `jit`/`--hex` path, делает fixture-v1 static decoder и
initial-RIP instruction-boundary compatibility checks, публикует и исполняет
один block ровно один раз, передаёт
`stop_rip=code_end` и не выполняет interpreter fallback/branch dispatch. V2
JIT остаётся unsupported. Child tests покрывают completed, budget, terminal
branch, недопустимую форму, internal branch без dispatcher, v2 denial и коды
выхода invalid/unsupported/harness. Независимый review нашёл и подтвердил
исправление compatibility-проверки initial RIP; повторные package checks прошли.

`BlockComplete` нормализуется сначала по фактическому RIP к `ReachedCodeEnd`,
затем по нулевому remaining budget к `InstructionBudgetExhausted`; другой
internal RIP означает `Unsupported`, так как для него нужен следующий block.
Это только одно-блочный execution path, не dispatcher и не сравнение с
физическим x86.

**Готово:** одна документированная команда реально исполняет IR-based block и выдаёт machine-readable result. Нулевой код возврата не появляется при fallback failure или отсутствии lowering.

### S3-14. Подтвердить register-only срез физически

1. Составить явный manifest программ, целиком исполняемых одним блоком: линейные формы и разрешённые terminal branches. Использовать подходящие неизменённые fixtures этапа 1 и собственные новые случаи; не требовать ещё отсутствующий dispatcher.
2. Сравнить interpreter, новый JIT и Mac mini native по определённому состоянию; повторить каждый case минимум дважды. Завершение native программы должно совпадать с выбранной границей сравнения.
3. Сохранить input, snapshots, fingerprints, host/toolchain и failures. Включить ABI/spill/branch boundary cases нового pipeline. Полный multi-block corpus этапа 1 запускается после S3-15.

Локальная подготовка добавила manifest
[`stage3_s3_14_jit_block.json`](../../tests/cpu/manifests/stage3_s3_14_jit_block.json):
8 исторических одно-блочных cases и 5 новых boundary inputs. Он исключает
`s0_09_sub_jnz_loop` (backward edge требует повторного dispatch) и
`s1_06_jz_not_taken` (fallthrough продолжает после Jcc в следующую инструкцию).
Новые случаи различают taken JZ/JNZ через валидный хвост до `code_end`,
not-taken JZ/JNZ с известной backward target boundary и fallthrough в
`code_end`, а также длинную цепочку helper calls с aliased ADD/SUB и
live-through значением. Целевой `od-jit` test на этой цепочке использует ту же
production allocation config и проверяет реальные value spill slots и helper
captures. Отдельный budget-boundary fixture в manifest отмечен
`local_only`: он завершает блок до `code_end` и не входит в физический набор.
Команда для будущего physical acceptance:

```sh
python3 tests/cpu/stage1_oracle.py --engine jit-block --manifest \
  tests/cpu/manifests/stage3_s3_14_jit_block.json --repeats 2 \
  --remote-root /ABSOLUTE/PATH/TO/ISOLATED_STAGE3_CHECKOUT
```

Legacy oracle сохраняет `jit` по умолчанию; `jit-block` без явных cases/manifest отклоняется,
а его результаты должны иметь backend `jit_ir_block`. Run manifest фиксирует
stage/suite, engine/backend и fixture/program digests; перед будущим oracle он
проверит что каждый обязательный fixture начинается с RIP 0 и ожидает
`reached_code_end`. Harness сравнивает interpreter непосредственно с выбранным
JIT, каждый из них — с native, и сравнивает каждый backend между повторами.
Orchestration tests проверяют выбор engine/backend, два повтора, отсутствие
implicit selection и failure при неверном backend tag. Все тринадцать manifest inputs локально совпали
между interpreter и `jit_ir_block`; вместе с harness tests это подтверждает
только локальную подготовку, не запуск Mac mini или triple physical acceptance.

Первый удалённый preflight для ревизии `980efd3` остановился на общем наборе
`od-isa`, `od-testkit`, `od-runner`, `od-x86-runner`: 706/711 тестов прошли,
пять известных x87/decode cases из [ISSUE-040](../KNOWN_ISSUES.md#issue-040)
упали до сборки и запуска native fixtures. Полный stdout/stderr этого запуска
не был сохранён. Для `jit-block` harness теперь запускает только suites
`od-testkit`, `od-runner`, `od-x86-runner` (один `od-isa` всё равно собирается
как dependency), а стандартный `jit` сохраняет прежний полный набор. Любой
результат remote preflight записывает полный доступный stdout/stderr и exit
code в run folder до продолжения или abort; timeout сохраняет partial output,
timeout status, limit и команду (exit code остаётся null). Harness tests
покрывают routing, nonzero exit и timeout.
После этой правки повторная доставка ревизии `b8924dc` в отдельный remote
checkout остановилась на SSH connect timeout. Native fixtures не запускались;
физическая приёмка оставалась открытой. Детали попыток записаны в
[`S3-14 report`](../reports/STAGE_3_S3-14_2026-10-26.md).

**Обновление, 3 ноября 2026 года:** на Mini `192.168.1.39` исходник коммита
`74d316f` был доставлен в отдельную директорию без Git history; локальный и
удалённый source fingerprint совпал
(`0f78441d1298009db38083318f9ac5e8821f4626e9f7f0f33efed16c2d41ada7`).
Стандартный CLI остановился на remote preflight `cargo test --offline
-p od-testkit -p od-runner -p od-x86-runner`: всего 571 passed, 42 failed,
exit 101; в завершившемся с ошибками binary `od-x86-runner` было 70 passed,
42 failed.
Отдельно обе сборки runner завершились успешно. Затем supplementary ручной
прогон применил те же manifest inputs, build identity, result validators и
сравнения: все 13 обязательных fixtures совпали между interpreter,
`jit_ir_block` и native x86, в каждом из двух повторов; repeat comparisons
дали 0 differences для всех трёх backends. Это только register-only
single-block/code-end evidence: бюджетный fixture, dispatcher, memory,
VMHost и широкий Stage 3 workload не запускались. Из-за неуспешного
обязательного preflight ручной результат не закрывает S3-14; triage отражён в
[ISSUE-051](../KNOWN_ISSUES.md#issue-051).
Полные результаты сохранены в [Mini evidence bundle](../reports/data/stage3/s3-14-mini-2026-11-03/).
На тот момент S3-14 оставался открытым; актуальный итог приведён ниже.

**Срез A:** тройное сравнение подтверждено; на момент этой записи обязательный gate ещё не был закрыт.

**Финальное обновление, 3 ноября 2026 года:** тестовые предположения,
вызвавшие 42 preflight failures, исправлены без изменения runtime-кода.
Повторный `cargo test --offline -p od-testkit -p od-runner -p od-x86-runner`
на Mini прошёл: 613 passed, 0 failed по 7 test binaries и 2 пустым doctest targets; `od-x86-runner`
прошёл 112/112.

Штатный `stage1_oracle.py` затем выполнил 13 register-only one-block/code-end
fixtures дважды на interpreter, `jit-block` и native x86; все pairwise и
repeat comparisons дали 0 differences. Локальный и удалённый source fingerprint
совпал (`7126851dcc2f2507ede25b7912cbab7fd7e84b6187c1b8973b38aa00b107f60e`).
Это закрывает физический gate S3-14. Более широкий dispatcher, memory, VMHost
и Stage 3 workload остаются за пределами этого среза; M1 Pro остаётся
отложен. Полный отчёт и evidence: [S3-14](../reports/STAGE_3_S3-14_2026-10-26.md)
и [Mini bundle](../reports/data/stage3/s3-14-mini-2026-11-03/).


## 7. Dispatcher, cache и память

### S3-15. Реализовать dispatcher и точный budget

1. После блока обработать next RIP, conditional/indirect target и выход без recursion по длинной цепочке блоков.
2. Считать guest instructions в JIT и fallback по единому contract. При недостаточном остатке budget использовать точный ограниченный путь, а не исполнять весь блок сверх лимита.
3. Проверить loop, zero/one budget, выход посреди блока, сохранение flags и bounded stack usage dispatcher.
4. Теперь прогнать все 10 ручных cases и фиксированный corpus из 1000 generated cases этапа 1 через новый runtime, interpreter и native oracle, минимум два повтора. Старые inputs/digests сохранить; сравнение только одного блока из программы больше не является достаточным.

**Готово:** multi-block program даёт состояние interpreter при том же budget; ошибочный target или невозможный exit не зацикливает runtime.

**Ограниченный generic путь S3-50…S3-55.** Все 31 регистровая
dispatch-обёртка проверена на эквивалентность общему входу. На допустимых
смесях native и helper проверены budget, STOP и состояние гостя.
Физическое сравнение полного корпуса этапа 1 для generic пути остаётся
открытым. [Итоговые локальные проверки](../reports/STAGE_3_CORE_REPORT.md#проверки-итогового-исходного-дерева-11cbe6b)

**Локальная partial реализация:** `od-jit` теперь имеет отдельный
`compile_block_for_dispatch`/`lower_for_dispatch` путь, который принимает
boundary witness только из полного непрерывного fixture-v1 decode, повторно
сверенного с теми же bytes. Dispatch API проверяет entry и все decoded block
ranges против этого admission. Typed `DispatchBranch` дополнительно
проверяется verifier по target, fallthrough, всем included IR records и точному
`code_end`. Runtime commit-ит выбранный Jcc RIP один раз и
возвращает `DispatchCandidate` (tag 8) с обычными guest counters. Legacy
`compile_block`/S3-13 runner не получают admission и сохраняют прежний
one-block `BlockComplete`/`NeedInterpreter` результат.

Следующий локальный partial срез добавляет `od_jit::dispatch::run` и
`od_host::PublishedBlockExecutor`. Dispatcher компилирует только текущий
admitted RIP, передаёт generated block точный remaining budget и проверяет
retired/remaining против входного budget и emitted instruction count. Он
завершает на `code_end`, возвращает budget/runtime exit, продолжает только на
admitted RIP после положительного retirement и отклоняет zero-progress loop.
Начальный `code_end` проверяется раньше нулевого budget; при budget=0
компиляция и execution callback не вызываются. Host executor публикует,
исполняет и уничтожает ровно один `PublishedCode` за вызов, до следующей
итерации; рекурсии и одновременно живых mappings нет. Целевые od-jit tests
покрывают двухблочный переход, budget forwarding, malformed counters,
unadmitted target, runtime unsupported exit и zero progress. Native od-host
integration test покрывает последовательные mappings и положительный
self-loop на macOS ARM64.

**Physical corpus gate, 3 ноября 2026 года:** Mini `192.168.1.39` прошёл
тот же frozen Stage 1 corpus: 10 manual и 1,000 generated inputs с seed
`0x5eedcafe`, corpus SHA-256
`9356e916b2037a3f5a082e8366fe33bf4277ab00812489809bfc586210744d35`, два
повтора. Все 2,020 fixture/repeat entries прошли на interpreter,
`jit-dispatch` и native x86; pairwise и repeat differences равны нулю, source
fingerprints совпали. Preflight: 613 passed, 0 failed. Durable summary:
[S3-15 Mini physical run](../reports/data/stage3/s3-15-mini-physical-run-2026-11-03.json).

Ограниченный checked-memory continuation добавлен отдельно в
`od_jit::memory_dispatch::run` и `od_host::PublishedMemoryBlockExecutor`.
Он работает только с двумя принятыми two-site artifact sequences и singleton
MOV qword load/store продолжениями, по immutable borrowed `CodeView` и явной
fixture execution boundary. Общий budget уменьшается по фактическому retirement;
STOP/stop-RIP, архитектурный second-site fault и host publication failure
сохраняют точный prefix state. Host executor освобождает каждый mapping до
следующей итерации. Unsafe caller contract сохраняет private synchronous RAM,
all-or-nothing write, no-MMIO/no-alias/no-concurrency требования; он не
подменяет capability. `od-machine` пока не содержит production owner, поэтому
owner-backed VMHost/MMU integration и owner-supplied code fetch остаются
открытыми. Явная fixture boundary — byte-window limit, не guest fetch; полный
code-fetch fault delivery остаётся непокрытым до owner integration. Две focused native differential проверки
сравнивают independent interpreter snapshots, budgets 1..=4, intra-pair и
inter-pair stop, data fault/resume without duplicate read, partial byte windows
at A+1/A+2/A+4, complete unsupported/invalid versus genuinely truncated decode,
cooperative STOP before entry/inside both pairs/after the final callback with
resume from actual RIP, unsupported tail и host error после первого committed
block. См. [memory-dispatch report](../reports/STAGE_3_S3-15_2026-10-27.md#bounded-checked-memory-dispatcher-partial).

#### S3-15p. Bounded arithmetic/checked-load production coordinator

`od_jit::memory_dispatch::run_bounded_arithmetic_load` is the public runtime
entry for the exact one/two arithmetic → `48 8B 03` → JZ/JNZ → existing ADD
fallthrough slice. It derives the complete plan from one immutable full
`CodeView`, keeps original guest RIPs, and treats prefix end only as an internal
handoff. Eager is the default strategy; demand-flags remains explicit and
uncached. One remaining budget and STOP/stop-RIP/event flow across native,
memory and suffix work. Restart accepts each decoded arithmetic boundary and
the Jcc/fallthrough boundaries, so a mid-prefix or post-Jcc yield can resume
without replay. Terminal native reports retain their actual retirement and
remaining budget. Typed errors cover plan/entry admission, compilation,
malformed counters, invalid budget exits, no progress, executor errors and phase RIPs. The
`ControlledBlockExecutor` publication hook lets host adapters resample control
after publication and before native entry; the published register mapping is
released before memory publication, and the checked-memory mapping before
suffix compilation. Host evidence and limitations are recorded in the
[S3-29e report](../reports/STAGE_3_S3-29e_2026-10-29.md). This remains a
borrowed fixture/API slice; owner-backed code fetch and MMU/RAM integration
remain open. The full S3-15 corpus/oracle gate and Mini physical run passed on
2026-11-03; performance acceptance remains open.

#### S3-15n. Production memory-owner integration prerequisites

The borrowed dispatcher remains a bounded fixture/API slice. Production
`od-machine` RAM/MMU ownership is a prerequisite and is not implemented by
S3-15n. Keep component responsibility explicit: `od-machine` owns RAM backing,
guest mappings, translation/permissions, code-source leases and committed-write
notifications; `od-jit` owns fetch/decode continuation policy, compile/cache/
dispatch and dependency invalidation; `od-host` owns executable publication
and mapping release. The dependency table and lifetime/fault contract are in
[JIT_CONTRACT](../specs/JIT_CONTRACT.md#production-memory-owner-integration-prerequisites-s3-15n).

The production path requires: (1) a synchronous private-RAM capability and
owner-backed code lease identifying byte ranges, backing/mapping identity and
generation, permissions and immutable lifetime; (2) owner-supplied incremental
fetch/decode so a later guest fetch fault is deferred until continuation
reaches its RIP, while compile errors remain host errors and fixture-window
end is not guest termination; (3) stop-RIP → STOP → budget → fetch ordering;
(4) one committed-write notification batch from every writer, including
aliases, mapping/protection changes and active-code dirty state, observed
before the next lookup/publication; (5) safe revocation and lease/mapping
release on all exits. Generic `GuestMemory` is not proof of private RAM or
revocation. Avoid adding trait layers until the concrete `od-machine` owner
exists and its required operations are known.

Acceptance must exercise permission and revocation, cross-page fetch, deferred
next-instruction faults after committed prefixes, all writer notifications and
aliases, stale-code suppression at the next safe point, plus publication and
lease release before another artifact. This prerequisite does not complete
S3-15: the full Stage 1 corpus, physical Mac mini comparison, and measured
cold/warm performance gates remain separate dependencies.

Следующий локальный CLI partial добавляет opt-in fixture-v1 engine
`jit-dispatch` и отдельный сериализуемый backend `jit_ir_dispatch`. Runner
повторно связывает полный decode с immutable fixture bytes, проверяет entry
boundary и вызывает итеративный dispatcher без fallback. Child tests на
macOS ARM64 покрывают taken/not-taken RIP, переходы между блоками,
backward/self-loop, budget внутри блока, budget-exact code_end, unsupported
decode/target и сохранность `jit`/`jit-block`. Fixture schema v1 запрещает
нулевой budget, поэтому его приоритет остаётся проверен API unit test. Это
не запускает 10-case плюс 1000-case corpus или физический x86 oracle;
на момент этой записи S3-14 физически оставалась открытой; после Mini rerun она закрыта (см. S3-14 report), а S3-15 открыта до полного acceptance
критерия выше. Проверки текущего partial slice прошли:
`cargo fmt --all -- --check`, `cargo check -p od-jit --all-targets --offline`,
`cargo test -p od-jit --offline` (64 tests), `cargo check -p od-host
--all-targets --offline`, `cargo test -p od-host --offline` (24 tests,
включая native iterative dispatch на текущем macOS ARM64 host),
`cargo test -p od-testkit --offline` (184 tests), `cargo test -p od-runner
--offline` (304 total: 101 unit, 4 jit-block CLI, 3 jit-dispatch CLI, 196 v2),
`cargo check --workspace --all-targets --offline`, `git diff --check` и
`graphify update .`. Независимый review CLI-среза чистый.

Harness для следующей проверки также поддерживает `jit-dispatch`: по умолчанию
выбирается 10 ручных cases плюс 1000 generated cases с seed `0x5eedcafe`, два
повтора. `--local-only` запускает только interpreter и `jit-dispatch`, сохраняет
frozen fixtures, сырые результаты и cross/repeat comparisons, а manifest явно
помечает `evidence_scope=local_only`, `physical_x86=not_run`. Этот режим не
засчитывается как physical acceptance. Для `jit-dispatch` удалённый Cargo
preflight использует `od-testkit`, `od-runner` и `od-x86-runner`; `od-isa`
остаётся dependency, но полный unit suite пропускается из-за пяти известных
x87 failures ISSUE-040. Полный физический режим сохраняет тройное сравнение и
metadata. Сам 10+1000 local-only прогон и его повторные результаты должны быть
отделены от hardware acceptance. На revision для этого среза local-only прогон
завершился успешно: 10 manual + 1000 generated inputs, два повтора, 4040
локальных engine results, включая 2020 interpreter↔dispatch comparisons и
2020 per-engine repeat comparisons, seed `0x5eedcafe`, corpus SHA-256
`9356e916b2037a3f5a082e8366fe33bf4277ab00812489809bfc586210744d35`.
Manifest: `work/stage1-oracle/stage1-20261027-000237-882135f1/manifest.json`;
summary: `work/stage1-harness/stage1-harness-20261027-000235-842de746/summary.json`.
Физический triple run не выполнялся и остаётся pending; локальное совпадение двух
собственных движков не закрывает S3-14/S3-15. Фактический локальный результат и
machine-readable summary записаны в
[`S3-15 report`](../reports/STAGE_3_S3-15_2026-10-27.md).

### S3-16. Подключить interpreter fallback

1. На форме без lowering материализовать state и вызвать реально поддерживающий её interpreter step. При отсутствии поддержки обоих вернуть unsupported.
2. Согласовать budget, STOP, fault и memory effects. После helper обновить context/code validity перед возвратом в JIT.
3. Защититься от нулевого прогресса и повторной компиляции того же неподдержанного entry без исполнения.
4. Учитывать compiled instructions, helper calls и interpreter steps раздельно в metadata/benchmarks.

**Готово:** смесь native blocks и interpreter instructions совпадает с чистым interpreter. Новые S2 формы используются только после их проверки; all-fallback workload не называется JIT-ускорением.

На локальном стенде macOS ARM64 закреплён отказ публикации в production
paged loop. При занятой MAP_JIT-аренде `MOV RAX,[RBX]` завершается одним
шагом интерпретатора. После освобождения аренды тот же сеанс доходит до
следующей итерации и выполняет загрузку нативно. Состояние, бюджет и
порядок записей совпадают с чистым интерпретатором. Обязательный шаг
`s3_16_publication_refusal` включён в local selector.
См. [JIT contract](../specs/JIT_CONTRACT.md#s3-16-paged-publication-refusal).
Локальный прогон `stage3-20261118-231721-cf43a57691` завершился
в статусе `local_only`: 345 шагов прошли, 40 остались `not_run`.
Новый шаг и все 153 самотеста стенда прошли.

**S3-55, bounded generic fallback:** opt-in mixed путь передаёт существующему
`od_isa::execute::step` точные bare CLD/STD и NOP, отдельно учитывает compile,
helper и успешные interpreter steps, проверяет progress и повторно проверяет
источник, CR3, context и зависимости после helper. Смеси с native blocks
проверены против интерпретатора. Общий Stage 2, profile-required и data-memory
fallback, а также production owner остаются открытыми; S3-16 не принята целиком.
[Итоговые проверки generic fallback](../reports/STAGE_3_CORE_REPORT.md#проверки-итогового-исходного-дерева-11cbe6b)

**Локальная partial реализация (S3-16):** добавлены отдельные
`S0_09NopAdmission` и `run_with_nop_fallback`. Admission строит полные
instruction boundaries по immutable `CodeView`, допускает S0-09 и только exact
bare `0x90`, проверяет branch targets по этим boundaries и отклоняет прочие
формы. Для NOP используется существующий `od_isa::execute::step` с fetch-only
memory, ограниченной одним байтом; x86 effects в JIT не повторяются.
`ControlledBlockExecutor` передаёт один `stop_rip` и тот же `AtomicU8 STOP` в
native runtime и pre-step guards. Native mapping освобождается до fallback.
Пер-часть native/interpreter retirement проверяется общей budget-инвариантой.
Tests покрывают all-fallback NOP, native prefix → NOP → native CMP/Jcc, flags,
budgets, stop order, отказ admission и terminal executor errors. Это отдельный
Rust API: fixture `jit-dispatch`, schema v1, legacy JIT и generic Stage 2
fallback не расширены. S3-16 остаётся открытой; эта узкая NOP проверка не
подтверждает общий fallback contract.

**S3-16m bounded-memory partial:** `memory_dispatch::run` остаётся строгим и
по-прежнему возвращает `Unsupported` на bare `0x90`. Новый opt-in
`run_with_nop_fallback` reuses the strict dispatcher with the remaining total
budget, and steps only the decoder-confirmed exact bare byte `90` through
`od_isa::execute::step` and a one-byte immutable fetch view. It returns separate
native/interpreter retirements and checks their sum against consumed budget.
The synchronous host executor releases `PublishedCode` before returning, so
the interpreter step follows native mapping drop; next native publication also
proves the single mapping can be reused. Tests compare the complete CPU state,
RAM and data callback counts against the full interpreter for budgets 0..=5,
plus STOP/stop-RIP before and after NOP, continuation without replaying the
first pair, unsupported tail preservation and strict-mode rejection. Prefix
NOP, `0F 1F`, PAUSE and all other non-exact forms never enter fallback. This
does not add production owner integration or general Stage 2 fallback; S3-16
remains open.

**S3-16n bounded-memory exact NOT RCX partial:**
`run_with_nop_fallback{,_controlled}` now accepts the existing exact bare NOP
and exact bare `48 F7 D1` (`NOT RCX`) at a strict dispatcher's `Unsupported`
RIP. A shared exact-decoder check requires the three-byte W64 register form,
RCX destination, no other prefix, and byte identity. The helper fetches only
those three immutable code bytes and uses `od_isa::execute::step`; strict
`run`, native admission, and all other unsupported forms remain unchanged.
The existing total-budget invariant, STOP/event/stop-RIP checks, boundary
priority, interpreter retirement, and actual RIP progress are reused. The
executor returns before the helper, preserving the synchronous native mapping
release boundary. A host fixture composes native pair → NOT RCX interpreter
step → native pair and compares full CPU/RAM state and accounting with the
interpreter at budgets 0..=5, stop-RIP boundaries before and after fallback,
atomic STOP after the native prefix, and unsupported NOT RAX neighbor behavior.
An od-jit regression preserves bare-NOP fallback when code ends at `u64::MAX`.

Rust identities are
`memory_dispatch::tests::not_rcx_fallback_is_exact_and_strict_run_stays_strict`,
`memory_dispatch::tests::nop_fallback_handles_code_end_at_u64_max`, and
`tests::bounded_memory_dispatch_not_rcx_fallback_composes_native_prefix_and_suffix`.
Harness gates `s3_16_not_rcx_fallback`, `s3_16_nop_u64_max_regression`, and
`s3_16_not_rcx_host_continuation` pin them, and
`test_s3_16_not_rcx_fallback_gate_is_pinned_and_fail_closed` checks omission,
zero-match evidence, and forged argv. Earlier S3-16n documentation recorded a
312-gate/140-self-test manifest snapshot; the manifest at local verification
contained 324 Rust gates (306 acceptance + 18 lifecycle), 342 local steps, and
149 self-test names. All three exact Rust gates and the fail-closed self-check
passed locally on ARM64. Mini and M1 checks remain deferred. This remains a
single fixture-bounded fallback form; S3-16 is still open, and production
owner/MMU integration is not covered. See the
[JIT testing reference](../testing/JIT.md#s3-16n-bounded-memory-exact-not-rcx-fallback).

**S3-16o / S3-34 mixed corpus partial:** the host test
`tests::s3_16o_s3_34_store_store_not_rcx_store_store_fixed_seed_boundaries`
composes exact native Store→Store (`48 89 03 48 89 02`), exact interpreter
fallback `NOT RCX` (`48 F7 D1`), and native Store→Store. Fixed seed
`0x5333_3136_6f34_636f` produces eight cases. Six budget boundaries (0 through
5), two stop-RIP boundaries, and callback-triggered STOP compare complete CPU
state, RAM, ordered write ledger, RIP, per-backend retirement, and remaining
budget against interpreter execution. Continuations verify committed writes
are not replayed. No instruction forms or production fallback paths were added.

The harness gate is
`s3_16o_store_store_not_rcx_store_store_corpus`; fail-closed self-test is
`test_s3_16o_store_store_not_rcx_mixed_corpus_gate_is_pinned_once_and_fail_closed`.
The self-check now pins all 149 unittest identities exactly once, including
three previously omitted process, S3-34bg, and S3-34bh methods found during
static review.
The earlier S3-16o report snapshot recorded 321 gates/146 self-test names;
current totals are 324 Rust gates (306 acceptance + 18 lifecycle), 342 local
steps, 324 distinct step IDs, 149 self-test names, and 18 Rust source pins.
Independent Sol/high review passed. The exact Rust gate and fail-closed
self-check passed locally on ARM64; Mini and M1 checks remain deferred. General
S3-16 and its production owner/MMU integration remain open. See the
[S3-16o implementation report](../reports/STAGE_3_S3-16o_2026-11-03.md).

### S3-17. Определить cache key и зависимости

1. Записать minimum relevant context key по разделу 4.3; включить все реальные compile-time assumptions и версии code/mappings.
2. Собрать зависимости blocks, пересекающих несколько regions/pages. Определить identity для aliases и повторного использования адреса памяти.
3. Проверить разные bytes/context при одинаковом RIP и одинаковые bytes при изменённой permissions/mapping identity.

**Готово:** stale или несовместимый block не считается hit. Выбор структуры поиска предварительно сравнен на representative key/workload; простой вариант допустим как измеренный baseline.

**S3-54/S3-55, bounded generic cache:** compile flavor разделяет strict и
mixed eager/demand пути; generic cache проверяет предоставленные владельцем
snapshot зависимостей и отклоняет несовместимый ввод. Production mapping и
backing identity/version, их связь с MMU и представительный выбор lookup не
подтверждены этой линией. S3-17 остаётся открытой.
Холодное сравнение S3-56 использовало исходную трассу S3-17 и все 1 010
входов с проверкой порядка и SHA-256. Исходный список путей остаётся
недоступным. [Привязка корпуса и результаты](../reports/STAGE_3_S3-56_S3-17_2026-11-19.md)

**Локальная partial реализация:** `od_jit::translation::TranslationKey`
требует typed compile flavor, entry RIP, полные bytes/base/end `CodeView`,
terminal classification, actual block instruction ranges и caller-provided
mapping/backing identity/version spans. Spans обязаны без дыр и overlap
покрывать весь CodeView; каждый span должен иметь execute permission, а версии
одной mapping/backing identity внутри snapshot должны совпадать. Разные mapping
identities могут alias один backing ID/generation. IDs и ненулевые версии
предоставляет владелец памяти; текущий `CodeView` не связан с реальным MMU и не
получает generation или identity из host pointer. Key использует полное
сравнение content при hash collisions. Full production cache/invalidation и
выбор структуры поиска не добавлены; отдельный one-entry S3-18a partial
описан ниже. Независимый `gpt-6.1-sol` reviewer на revision
`629b1b14a341e47758e7dd9f1da11f7abf5a68cd` подтвердил fixture-level key,
dependency и lookup acceptance; проверены bytes/ranges, aliases,
mapping/backing identity/generation, executable permissions, hash-collision
equality и validation order. Это закрывает reviewer-acceptance gate, но не
всю S3-17: owner-provided identity/version guarantees, production
cache/invalidation и выбор production lookup policy остаются открытыми.
Исторические Vec/HashMap timings относятся к source revision `85afb316…` и
не являются замером текущего `ExecutionContextSnapshot` key или end-to-end JIT.
Подробности и provenance — в [S3-17 review acceptance report](../reports/STAGE_3_S3-17-review-acceptance-2026-10-29.md).

**Benchmark prototype:** `tools/bench` содержит ARM64 macOS `record` wrapper
вокруг реальных `dispatch::run` block-entry вызовов и portable `measure`
replay. Replay заново строит keys вне timing и сравнивает `Vec` с
`std::collections::HashMap` при одинаковом полном key equality и
`dependencies_match_snapshot`. Отдельно предусмотрены cold construction+insert
и warmed lookup+dependency validation; сценарии включают recorded trace,
key-size ladder/observed count, hot loops, unique sequence, mixed hit/miss,
stale mapping/backing/permission, aliases и 1/2/4 dependency spans. Это только
прототип harness. На frozen corpus выполнены три release process runs; 18
сценариев и сырые результаты описаны в [отчёте измерений](../reports/STAGE_3_S3-17-lookup-measurements-2026-10-26.md).
Эти table-level результаты не являются end-to-end JIT speedup и не выбирают
production lookup/cache policy. Reviewer-acceptance gate закрыт на revision
`629b1b14a341e47758e7dd9f1da11f7abf5a68cd`; S3-17 остаётся открытой для
owner-provided identity/version guarantees, production cache/invalidation и
решения о production lookup policy. См. [отчёт reviewer acceptance](../reports/STAGE_3_S3-17-review-acceptance-2026-10-29.md).
Ограничения и команды
описаны в [протоколе измерений](../testing/BENCHMARKS.md#s3-17-translation-key-lookup-prototype).

### S3-18. Реализовать ограниченный cache

1. Добавить lookup, compile miss, publish и reuse; проверить версии до вставки и входа в block.
2. Задать capacity по blocks/code bytes/metadata и явный путь при исчерпании. Не допустить неограниченного роста при уникальных RIP.
3. Считать hit/miss/compile/publication и bytes. До S3-25 освобождать только заведомо неактивные entries; активные сохранять до безопасной точки.

**Готово:** повтор программы использует block без новой compile/publication, capacity корректен, изменённый dependency не попадает в execution.

**S3-54, bounded generic cache:** FIFO/LRU, capacity и проверка зависимостей
используют существующий multi-entry cache. На ARM64 повторный вызов выполняет
тот же опубликованный handle без новой компиляции и публикации; проверены
инвалидация и смена контекста между синхронными вызовами. Production owner,
MMU, memory-путь и полная этапная приёмка остаются открытыми; S3-18 не принята.
На трассе S3-17 FIFO и generic дали одинаковые 2 hits, 1 320 компиляций и
публикаций и 1 304 eviction на 1 322 visits. Пиковые размеры кода и метаданных
также совпали. [Счётчики на полном S3-17](../reports/STAGE_3_S3-56_S3-17_2026-11-19.md)

**S3-18a local partial:** `od_jit::dispatch::run_cached` adds one resident
fixture-dispatch block with an opaque published handle supplied by a generic
backend; `od-host` implements the backend without changing existing
release-on-return executor contracts. Same-key hits rebuild/validate the
bounded guest record and full `TranslationKey`, then skip lowering, allocation,
emission and publication. Miss replacement drops the old published owner
before publishing the candidate. The cache rejects capacities other than one
and caps logical key/code/state-map/publication-copy contents at 512 KiB;
allocator capacity, page rounding and host-handle internals are explicitly
outside that logical bound. Dependency/source validation occurs on hit, before
publish and before entry; a lower bound rejects oversized source/dependency
payloads before key copying. Stale input returns an error without retry. STOP
is invocation-local and resampled at guest boundaries; budget counters remain
per guest instruction. Mock tests cover same-key reuse, self-loop/budget, A→B replacement
and drop ordering, dependency changes, invalid snapshots, oversize candidates,
publication errors, clear/drop lifetime and zero-budget/code-end priority. A
native od-host fixture compares cached and uncached state/results and repeats a
cache hit. This S3-18a subsection describes only the one-entry policy and does
not integrate the separate host arena described below. One-entry hit, miss,
compile, publication and replacement counters are available for measurement;
S3-18c adds multi-entry hit/miss/compile/publication/eviction and resident-byte
counters. Memory lowering, production MMU
identity/invalidation, and benchmark evidence remain absent. S3-18 stays open;
S3-22/23/25 remain separate context, invalidation and retirement work.

**S3-18b host arena partial:** `od-host::PublishedArenaBackend` owns one
`MAP_JIT` mapping with a fixed slot size of 4096 bytes and a bounded capacity of
1..=16 blocks. Mapping length is checked and rounded using the runtime page
size. A preallocated free-slot stack tracks slots; each
`PublishedArenaBlock` handle retains the arena, slot generation and PC map.
The arena uses the same global `MapJitLease` as `PublishedCode`, so concurrent
standalone/another-arena publication returns typed `MappingAlreadyLive` until
the last backend or handle drops. Code publication validates the slot length
and metadata, temporarily enables current-thread JIT write access only for the
slot copy, restores protection, and performs instruction-cache invalidation
before exposing the handle. Slot exhaustion does not consume a slot; there is
no per-slot mmap/munmap, relocation or compaction. Arena calls reuse the shared
verified call/result decoder. Native tests cover two simultaneous live blocks,
full capacity, reuse of a dropped slot while another block remains executable,
foreign handles, owner/handle lifetime and checked size overflow. The 16-slot
limit is a provisional capacity choice without performance evidence. This host
publication API does not alter S3-18a's one-entry cache. S3-18 remains open.

**S3-18c multi-entry cache partial:** `od_jit::dispatch::MultiEntryBlockCache`
is a separate opt-in path using the shared cached-dispatch control loop and an
opaque `CachedBlockBackend` handle. `PublishedArenaBackend` reports slot
capacity through `MultiBlockCachedBackend`; the runner rejects a cache whose
configured entry count exceeds that capacity before compile or publication.
The cache uses insertion-order `Vec` lookup with FIFO eviction; hits do not
refresh order. Its hard limits are 1..=16 entries, 65,536 actual emitted code
bytes total, 512 KiB aggregate logical entry/key/PC-map metadata, and the
existing 4096-byte per-block compiler cap. Individual over-limit candidates
are rejected without eviction. A miss is compiled and source/dependency
validated before capacity eviction; inactive oldest handles are dropped before
publication. Publish failure leaves those evictions in effect. A stale check
after publication drops the candidate without execution and does not retry.
Counters expose hits, misses, successful compiles/publications, evictions and
resident code/metadata bytes. Tests cover A→B→A, self-loop/budget, sixteen-key
reuse, exact seventeenth-key FIFO victim, metadata-driven multi-eviction,
oversized candidates, malformed dependency rejection, stale post-publication
handle drop, publication failure, capacity
mismatch, and accounting bounds. A native od-host fixture compares cached
arena execution with uncached execution and verifies reuse. This remains
partial: S3-18d1's three bounded cache measurement runs are recorded below,
but they do not establish a policy conclusion. Production MMU
identity/invalidation, memory lowering, or S3-22/23/25 lifecycle work is not
claimed. S3-18 remains open.

The recorded 16-key/FIFO/LRU measurements are one bounded fixture observation.
More representative end-to-end workloads and resource evidence are required
before choosing policy or capacity; the current implementation makes no
optimality or product speedup claim.

**S3-18d1 bounded measurement partial:** `od-bench dispatch-cache` adds an opt-in
native Apple Silicon end-to-end harness for uncached publication, one-entry
arena cache, FIFO(16), and LRU(16). It accepts only the exact frozen S3-17
case-list/trace fingerprints, checks full fixture-v1 decode admission and
recorded entry boundaries, and prepares the interpreter oracle before timing.
Deterministic scenarios cover the 1010-case cold sweep, two/four warmed corpus
repeats, hot 1/8/16 single-block sets, a recorded multi-block input, a 17-key
capacity edge, and hot8/hot16 followed by a distinct insertion. Variants share
the calibrated repetition count and therefore execute equal operation counts
in paired samples; each sample must accumulate at least 200 ms or the run
fails without a result. The JSON report records selected cases, 20 paired raw
samples, per-sample counters/high-water, arena reservation size, summaries and
paired ratios, and is written atomically. An opt-in LRU seam was added;
`MultiEntryBlockCache::new` still selects FIFO. Three release process runs were
collected on 2026-10-30. See [S3-18d1 measurement report](../reports/STAGE_3_S3-18d1_2026-10-30.md).
The bounded fixture measurement does not establish a cache-policy/default or
product speed conclusion. S3-18 remains open; S3-22/23/25 remain separate work.

**S3-18e checked-memory cache partial:** Commit
`b1a6ccb130844c40a396155a8f84bda656bfa8c3` adds an opt-in capacity-one
`MemoryPairCache` for only the frozen `StoreThenLoad` and `LoadThenStore`
profiles. Its key covers the complete immutable source view, entry RIP,
validated executable dependency identities/generations/permissions, profile,
and caller-owned execution-context generation. It limits dependency spans
before key allocation and applies a 512 KiB logical bound. Same-key use
revalidates the key; clear or replacement releases the previous inactive
published handle before new publication. The retained `od-host` handle contains
code and immutable artifact metadata only; CPU/RAM/access context/budget/STOP
and event controls are supplied anew for each synchronous execution. Focused
unit and native host tests cover reuse, replacement, failure cleanup, guards,
control-flow and full-state/memory parity against uncached dispatch. This is
fixture-only evidence under private synchronous ordinary-RAM assumptions; it
does not implement production owner/MMU/SMC/DMA lifecycle or establish a
performance/default-policy conclusion. See the
[S3-18e implementation report](../reports/STAGE_3_S3-18e_2026-11-01.md) and
[JIT test scope](../testing/JIT.md#s3-18e-bounded-checked-memory-cache).
S3-18 remains open.

**S3-18f exact Load→Load retained profile partial:** adds only the existing
S3-26e bytes `48 8B 03 48 8B 0B` to the typed capacity-one cache admission.
The full key, code-only retained handle, 512 KiB bound, and replacement
lifetime are unchanged; each hit receives fresh state, memory, access context,
STOP/event controls, budget, and stop-RIP. Two same-address reads remain
independent; a second-read fault commits the first result at `A+3`, and a
continuation does not replay it. An entry beginning at `A+3` remains rejected,
and `StoreThenStore` is still rejected. Sol/high static review passed. The
exact host regression now passes after ISSUE-048/032 were resolved. Stage 3
acceptance remains partial pending deferred Mini and stage-wide checks. This
adds no benchmark/default-policy or performance claim. See the [S3-18f report](../reports/STAGE_3_S3-18f_2026-11-03.md),
[JIT test scope](../testing/JIT.md),
and [ADR-0004](../decisions/ADR-0004-capacity-one-memory-cache.md).
S3-18 remains open.

### S3-19. Подключить checked memory bridge

1. Использовать существующий GuestMemory/AccessContext и подтверждённые fault outcomes этапа 2 через узкий generated helper ABI.
2. Различать fetch/read/write, address overflow/backend error и architectural fault; не добавлять альтернативную MMU в JIT.
3. На первом пути все memory operations checked. Direct host-memory fast path рассматривать только отдельно после проверки ownership, mappings и замеров.

**Готово:** helper success/fault/internal error сохраняют правильный state и exit reason, не обходят текущие ограничения CPU contract и ISSUE-012.

#### S3-19a. Invocation-local helper ABI partial

`od-jit::memory_bridge` добавляет invocation-local `CheckedMemoryAdapter` над
`&mut dyn GuestMemory` и отдельный fixed-width `CheckedMemoryHelperV1`. Его
status namespace — `0 Success`, `1 ArchitecturalFault`, `2 MemoryError`,
`3 InvalidArgument`; он не является `RuntimeExitTag` и не меняет
`RuntimeContextV1` (включая зарезервированный invalid tag 6). Error payload
передаётся только фиксированными C-полями: architectural vector/error code/
linear address либо уточнение boundary/backend ошибки; `MemoryError::Backend`
text не пересекает ABI.

Первый профиль разрешает fetch шириной 1 и integer read/write шириной 1/2/4/8.
Read destination меняется только после успешного backend read; write вызывает
один `GuestMemory::write`. Adapter не владеет CPU state/counters, не вычисляет
effective address и не выполняет lowering. Native ARM64 od-host test строит
probe через checked encoder, напрямую вызывает helper и проверяет success,
staged load candidate/retirement sentinel commit только при success и отдельный
backend-error status с сохранением этих sentinels при ошибке; unit tests bridge
проверяют один backend write.
`S3-19a` не реализует block exit/fault replay, production integration, MMU,
MMIO, RMW/atomic, REP, descriptor accesses, direct host-memory access или
lowering; полный S3-19 и S3-20 остаются открытыми.

#### S3-19b / S3-20a. Один production V2 MOV load

Отдельный `compile_memory_load_block_v2` использует единственный источник
decoded semantics — `od_isa::long_decode` — и выдаёт приватный compiler-issued
V2 artifact только для exact `48 8B /r`, ModRM `mod=00`, destination/base
RAX..RBX. Он не допускает SIB, displacement, segment prefix или address-size
override и останавливает native block после этого одного load. В ARM64-коде
нет вычисления effective address: код передаёт site index доверенному Rust
helper, который вызывает `MemoryOperand::access_address_48`, затем
`CheckedMemoryHelperV1`.

V2 имеет отдельные `RuntimeContextV2`/`BlockEntryV2`, compile flavor и host
publish/execute APIs; V1 entry остаётся отдельным и отклоняет V2 до pointer
cast, tag 6 в V1 остаётся invalid. Успешный read сначала записывается в staging,
после чего одним commit меняются destination, RIP и retirement; fault и
boundary/backend errors сохраняют canonical state на faulting RIP и
различаются по результату host API. `RAX` alias и non-alias формы проверены на
ARM64 native path вместе с fault payload, boundary/backend separation,
budget/STOP/stop-RIP. Native success, architectural-fault preservation и
boundary error сравниваются с `od_isa::execute::step` на независимых copies
CPU/RAM, включая полный CPU state, RAM bytes и read count. Этот путь не использует общий IR/allocator, не имеет
PC-map, cache или multiblock integration; source bytes/decoded site/compiler
provenance остаются частью отдельного V2 artifact. Memory pointers и RAM
contents invocation-local и не входят в translation key.

#### S3-20b. Один production V2 MOV store

`compile_memory_store_block_v2` принимает ровно одну инструкцию `48 89 /r`,
ModRM `mod=00`, source/base RAX..RBX, qword; SIB, displacement и любые
дополнительные байты/prefixes отклоняются. Store использует тот же decoded
site/helper/emitter family, отдельные source/destination metadata, compile
flavor, artifact/publication type и host entry. Load entry отклоняет store
artifact до native call.

Store доступен только через unsafe `execute_memory_store_v2`: caller обязан
предоставить private synchronous ordinary RAM с all-or-nothing write, без MMIO,
DMA, конкурентных или отложенных эффектов; backing не исполняется и не
алиасит cached guest code, и не становится executable до drop compiled block.
Вызов helper делает один checked write после захвата address/source; ошибка не
меняет RAM или canonical CPU state, success затем коммитит RIP/retirement и
только после этого читает STOP. `MOV [RAX],RAX`, little-endian bytes,
interpreter differential success/fault/backend-boundary cases и поздний STOP
проверены локально на независимых snapshots; entry STOP/stop-RIP/zero-budget
guards возвращаются без записи. Code writes не допускаются до
S3-23. Это не общий memory lowering и не cache/dispatch integration.

Stores beyond this exact isolated slice, other address forms, general memory
lowering and dispatch/cache integration remain open. Локальные Rust/ARM64 проверки разрешены;
физические Mac mini/x86 проверки не запускать до финального S3-21 gate после
завершения остальных локальных задач Stage 3.

#### S3-20c. Один SIB byte для V2 MOV load/store

Обе production V2 qword операции принимают ровно четырёхбайтовую форму
`48 8B/89 /r sib`: ModRM `mod=00,r/m=100`, data/base/index RAX..RBX,
scale 1/2/4/8, без displacement, лишнего prefix или хвостовых байтов. Index
`100` (no-index), base `101` (no-base/disp32), RSP/RBP и R8+ формы запрещены.
Существующие direct-base трёхбайтовые encoding остаются допустимы.

`long_decode::decode` остаётся единственным декодером; компилятор сверяет raw
ModRM/SIB биты с decoded length/operation/register/base/index/scale, но не
вычисляет effective address. Source identity хранит bounded `[u8; 4]` плюс
length, а site использует существующий `MemoryOperand::access_address_48` с
wrapping и canonical48 правилами. ABI, V2 tag, load/store publication/API,
store safety и commit ordering не меняются.

Локальные ARM64 native differential tests сравнивают все 256 валидных
data-register/base/index/scale комбинаций отдельно для load и store с
`od_isa::execute::step` на независимых полных CPU/RAM snapshots. Проверены
little-endian store bytes, aliases, wrap-to-canonical, noncanonical guest fault,
backend boundary error и отсутствие эффектов при STOP/stop-RIP/нулевом budget.
Invalid/truncated/tail/displacement/no-index/no-base/extended/prefix формы
отвергаются. Срез описан в [S3-20c report](../reports/STAGE_3_S3-20c_2026-10-27.md).
Физическая проверка Mac mini остаётся отложенной до финального S3-21 gate.

#### S3-20d. Signed displacement в direct и SIB V2 MOV

Обе qword операции `48 8B/89 /r` допускают ModRM `mod=00`, signed disp8 при
`mod=01` и signed disp32 при `mod=10`, с существующей direct-base или
однобайтовой SIB формой. Точные длины без SIB: 3/4/7 bytes; с SIB: 4/5/8
bytes. Base, index и data register ограничены RAX..RBX; scale SIB — 1/2/4/8.
Mod11, RIP-relative, base-less, no-index, RSP/RBP/R8+, любые иные prefixes,
address-size override и trailing bytes отвергаются.

`long_decode::decode` остаётся единственным декодером и задаёт `MemoryOperand`,
signed displacement и `next_rip`. Immutable artifact identity хранит полный
bounded source `[u8; 8]` и длину. Общий `MemorySiteV2` validator проверяет
decoded site shape и согласованность raw ModRM/SIB/source length, не вычисляя
EA и не дублируя sign extension. Runtime использует прежний
`MemoryOperand::access_address_48` для wrapping, canonical48 и access-range
правил. ABI, V1, checked helper, load/store safety и commit ordering прежние.

Локальные native differential tests перебирают допустимые base/index/data/scale
и mod формы для load/store, включая disp8 `-128/-1/0/1/127` и disp32 min/max
с соседями. Проверяются одинаковые адреса у эквивалентных disp8/disp32 при
разных `next_rip`, aliases, wrap-to-canonical, final noncanonical и endpoint
faults, boundary/permission/backend errors, no-effect guards и поздний STOP
после одного store. Результат описан в
[S3-20d report](../reports/STAGE_3_S3-20d_2026-10-27.md). Mac mini gate остаётся
только финальным S3-21.

#### S3-20e. RIP-relative V2 MOV

Добавлен узкий RIP-relative профиль для qword `48 8B/89 /r`: ModRM
`mod=00,r/m=101`, data register RAX..RBX, signed disp32 и ровно семь байтов
инструкции. `long_decode::decode` остаётся единственным декодером; immutable
site связывает все source bytes, decoded `MemoryOperand` и `next_rip`.
Runtime принимает только state с `RIP == site.guest_rip` и вызывает
`MemoryOperand::access_address_48(state, site.next_rip)`; emitter не вычисляет
адрес и host path не перебазирует instruction.

Compiler coverage проверяет обе операции, все четыре data registers и
`i32::MIN/-8/-7/-1/0/1/8/i32::MAX`, включая site после двух байтов внутри
CodeView. Native/interpreter differential проверяет те же байты на двух
guest RIP для всех пар operation/register/displacement, сравнивая полный CPU
state и RAM; случаи вне fixture RAM или с store в executable code не коммитят
изменения. Поддержка не распространяется на другие RIP формы, prefixes,
trailing bytes или общую IR/cache интеграцию. Результат и фактические проверки
записаны в [S3-20e report](../reports/STAGE_3_S3-20e_2026-10-27.md). Mac mini
проверка остаётся только финальным S3-21 gate после локальной работы Stage 3.

#### S3-20f. Address-size override V2 MOV

Отдельная admission ветка принимает только `67 48 8B/89 /r`: ModRM
`mod=00`, RAX..RBX в data и base полях, qword data, dword address size,
без index/SIB/displacement, длина ровно четыре байта. `long_decode::decode`
остаётся единственным decoder; runtime вызывает
`MemoryOperand::access_address_48` и не дублирует zero extension или mask.
Старые direct/SIB/displacement/RIP profiles не расширены этой веткой.

Local native/interpreter differential покрывает 16 data/base сочетаний для
обоих направлений, включая верхние биты базовых GPR, полные qword значения,
`MOV RAX,[EAX]` и sparse successful qword access from `0xffffffff` through
`0x100000006`. Dword address-size truncation applies to the starting effective
address only; operand byte addresses then advance linearly without 32-bit
wrap. A separate unmapped case checks fault atomicity. Truncated, trailing,
prefix, mod01/mod10/mod11, SIB и base-less формы
отвергаются. Отчёт: [S3-20f](../reports/STAGE_3_S3-20f_2026-10-27.md). Mac mini
проверка остаётся финальным S3-21 gate.

#### S3-20g. Signed displacement при address-size override

V2 load/store дополнительно принимают `67 48 8B/89 /r disp8|disp32` с
ModRM `mod=01/10`; data/base RAX..RBX, длины ровно 5/8 байт. Существующий
`mod=00` остаётся доступен. SIB, RIP-relative/base-less форма, другие
префиксы и регистры остаются запрещены. Decoder по-прежнему единственный
источник `MemoryOperand`, signed displacement и `next_rip`; raw-byte admission
сверяет только encoding и decoded metadata. Lowering/helper не вычисляют EA и
не повторяют sign extension: адрес и полная qword-проверка идут через
`MemoryOperand::access_address_48`.

Локальные native/interpreter differential cases покрывают disp8
`-128/-1/0/1/127`, границы disp32 и соседние значения, верхние биты base,
32-bit EA wrap и alias destination/source с base. Для qword от `0xffff_fffc`
проверено линейное продолжение до `0x1_0000_0003`; отсутствующий последний
байт сохраняет CPU/RAM для load и store. Truncated/trailing bytes, SIB,
неподдержанные регистры и дополнительные/переставленные prefixes отклоняются.
Отчёт: [S3-20g](../reports/STAGE_3_S3-20g_2026-10-27.md). Проверка Mac mini
остаётся частью финального S3-21 gate.

### S3-20. Добавить memory lowering

1. Начать с проверенных S2 MOV load/store forms, затем подключать остальные формы по готовности interpreter/native.
2. Правильно вычислять guest effective address, widths, partial-register writes и порядок accesses. Не повторять decoder semantics вручную в emitter.
3. Сохранить commit ordering: fault не оставляет запрещённую запись в register/flags/RIP, successful store уведомляет memory owner о side effect.

**Готово:** memory fixtures с boundary/unaligned/alias cases совпадают локально с interpreter; до S3-21 это не физическая приёмка памяти.

#### S3-20h / S3-26a. Один checked qword load через общий IR runtime path

Отдельный opt-in builder допускает ровно `48 8B /r`, ModRM `mod=00`, data и
base RAX..RBX, без SIB/displacement. `CheckedLoad` — typed IR effect с
qword `MemoryOperand` и success-only temporary; verifier связывает его с
единственной текущей instruction, валидным descriptor, `WriteGpr` этого
temporary и commit после write. Lowering получает операнд только из decoder.

Allocator видит реальную helper boundary, планирует spill/capture и restore;
общий runtime emitter вызывает существующий checked-memory helper по site
index 0, оставляет результат staged до GPR/RIP/counter commit и строит общую
PC map с helper-call и commit границами. Новый compile flavor, compiler-issued
artifact, опубликованная metadata и `RuntimeContextV2` entry имеют отдельные
типы. V1 executor и прямой V2 load API отвергают этот artifact до native
entry. Существующий прямой V2 compiler остаётся самостоятельным baseline.

Локальный native ARM64 test сравнивает alias load `[RAX] → RAX` с прямым V2 и
interpreter по полному CPU state и памяти. Также проверены qword success,
retirement, architectural fault с неизменным destination/flags/RIP/state,
backend error, STOP, stop-RIP, zero budget, verifier negatives, и границы
helper/commit в PC map. Это один bounded fixture slice: нет общего multiblock
memory lowering, stores, MMIO, atomic операций или автоматического host fault
recovery. См. [S3-20h/S3-26a report](../reports/STAGE_3_S3-20h_2026-10-27.md).

#### S3-20i / S3-26b. Один checked qword store через общий IR runtime path

Отдельный opt-in builder допускает ровно `48 89 /r`, ModRM `mod=00`,
source/base RAX..RBX, без SIB/displacement. `CheckedStore` явно хранит guest
source GPR и decoder `MemoryOperand`; verifier требует единственный store и
следующий непосредственно за ним commit, отвергая дублирование и эффекты до
commit. У store отдельные compile flavor, artifact, публикация и invocation
entry. V1, direct V2 load, IR load и direct V2 store APIs отвергают этот
artifact до native entry.

Generated entry исполняет stop-RIP → STOP → budget guards, один checked helper
call, затем сразу фиксирует RIP/budget/retirement; лишь после полного commit
читает STOP. Store, завершённый при поднятии STOP внутри callback, сохраняется
и возвращает `StopRequested`. Helper errors сохраняют CPU/RAM и не retire-ят.
Host entry сохраняет те же unsafe private synchronous ordinary-RAM
предпосылки, что direct V2 store.

PC map помечает helper call и путь успешного возврата к commit как
`InadmissibleAfterEffect`, поэтому неизвестный host fault не может быть
превращён в replay guest store. Ошибка helper остаётся planned/replayable,
поскольку helper не меняет RAM при ошибке. Native ARM64 fixture сравнивает IR,
direct V2 и interpreter на независимых snapshots `MOV [RAX], RAX`, проверяет
ровно один callback/write, commit/retirement, STOP после write, architectural и
backend errors, pre-entry guards, verifier negatives, PC-map и typed-entry
rejection. Ограничения: одна инструкция, private ordinary data RAM; без MMIO,
DMA, concurrency, atomics, executable aliases, active SMC, multiblock порядка
или performance claim. Факты записаны в
[S3-20i/S3-26b report](../reports/STAGE_3_S3-20i_2026-10-27.md).

#### S3-20j / S3-26c / S3-20k / S3-26d. Один generated entry для двух точных memory sequences

Отдельный opt-in artifact допускает ровно две sequences: `48 89 03 48 8B 0B`
(`MOV [RBX], RAX; MOV RCX, [RBX]`) и `48 8B 03 48 89 02`
(`MOV RAX, [RBX]; MOV [RDX], RAX`). Builder/lowering сохраняют обе instruction
records в одном `BasicBlock`/IR; verifier связывает каждую memory operation,
GPR write и commit с активной instruction. Typed artifact хранит выбранную
sequence и `[site0, site1]`; общий emitter проверяет и исполняет ровно одну из
этих форм. Single-site helper/publication/execution APIs сохраняют проверки
index 0 и отвергают двух-site artifacts.

Один native entry запускает оба site последовательно и допускает повторный
вход только на RIP второй инструкции. Store→load сохраняет первый store при
ошибке load; load→store сохраняет RAX после load при ошибке store. Второй site
может fault и на continuation с нулём retired в этом вызове, не повторяя первый
site. Guards второго instruction имеют порядок stop-RIP → STOP → budget.
Helper/fault/commit PC map ranges сохраняют site и guest instruction; store
helper/commit остаётся non-replayable.

Локальная native ARM64 fixture сравнивает state/RAM с двумя interpreter steps
на независимых copies и проверяет callback order, fault payload/partial commit,
STOP, budget, continuation после изменения source RAM, guards и typed entry.
Slice остаётся private synchronous ordinary RAM без branch, code aliases, MMIO,
atomics, произвольных multi-site blocks или performance claim. См.
[S3-20j/S3-26c/S3-20k/S3-26d report](../reports/STAGE_3_S3-20j_2026-10-27.md).

#### S3-26e. Exact load→load two-site checked-memory entry

Add exactly `48 8B 03 48 8B 0B` (`MOV RAX, [RBX]; MOV RCX, [RBX]`) to the
existing typed two-site path. Keep two helper sites even though the effective
address is equal: guest callback order must be read(A), read(A), and the second
read must not be coalesced. Both loads have independent GPR commits and exact
RIP boundaries `A`, `A+3`, and `A+6`. The shared entry accepts the first RIP or
the second-instruction continuation only.

Acceptance compares full CPU/RAM state with two interpreter steps and records
both callback address/width pairs. Cover first-site fault, second-site fault
after committing RAX, a second-site backend error, and singleton load retry at
`A+3` without repeating the first read. Check budget 0/1/2, pre-entry STOP,
stop-RIP at the second instruction, STOP from the first callback, and the
existing S3-28b event boundary: when the first read raises an event, both reads
finish before yield if STOP/fault do not terminate earlier and budget remains.
The event stays pending and yields before a following instruction. Keep all
other memory-pair forms rejected. This is private synchronous ordinary RAM;
the read callback trace is a contract probe, not an MMIO/device model. Update
the local S3-34e lifecycle list with the exact host test identity. Full S3-26,
S3-27, owner/MMU behavior and Stage 3 remain open. See the
[S3-26e report](../reports/STAGE_3_S3-26e_2026-10-30.md).

#### S3-26f. Exact store→store two-site checked-memory entry

Added only `48 89 03 48 89 02` (`MOV [RBX],RAX; MOV [RDX],RAX`) to the existing uncached V2 pair path. The source bytes bind site 0 to `[RBX]` at `A` and site 1 to `[RDX]` at `A+3`; both use RAX as the payload. The shared typed lowering emits two ordered `CheckedStore` operations and separate commits. The runtime emitter binds the second store helper to operation index 4 and keeps commit boundaries at `A+3` and `A+6`; equal effective addresses still cause two write callbacks.

The added evidence covers independent interpreter parity, first-write no-commit, second-write architectural fault with the first write retained and unchanged GPR/flags, exact second-fault payload and continuation/singleton retry without repeating site 0, backend errors as host errors, budgets 0/1/2, entry STOP, stop-RIP `A+3`, callback-raised STOP and fault precedence, PC-map phases/boundaries, malformed and neighboring source rejection, uncached pair dispatch and second-site continuation, and explicit rejection by the cached pair backend. Each write retains the private synchronous ordinary-RAM all-or-nothing contract; the pair itself is not a transaction. Production owner/MMU, MMIO, multi-vCPU and physical-x86 semantics are outside this slice. A missing exact first-site builder admission arm was repaired; all four affected od-jit gates and the complete 397-test od-jit suite pass. Exact host lifecycle, corpus and testkit gates also pass; independent Sol/high review passed. These focused runs do not establish full Stage 3 or production owner/MMU acceptance. See the [S3-26f report](../reports/STAGE_3_S3-26f_2026-11-03.md).

#### S3-20l. RIP-relative MOV in shared IR and bounded dispatch

The existing decoded `MemoryOperand` is carried unchanged through
`CheckedLoad`/`CheckedStore`; effective-address semantics continue to come from
`access_address_48` at that instruction's `next_rip`. The opt-in singleton
profile accepts only exact seven-byte `48 8B/89 /r disp32`, ModRM
`mod=00,r/m=101`, qword data register RAX..RBX. Shared-IR verification, typed
host publication/execution validation, and `memory_dispatch::compile_at` admit
the same shape. A truncated explicit byte window returns `BoundaryReached`
before publication or a data-memory callback. Existing direct-base and pair
profiles retain their exact admission.

Native ARM64 tests cover direct V2/shared IR/interpreter parity for negative,
zero and positive displacement and a CodeView entry after a prefix. The
dispatcher composes this singleton, the existing bare-NOP interpreter step,
and an existing two-site pair in both orders; it checks budgets 0..4,
actual-RIP continuation, and 1..6-byte partial windows. Shared-IR load/store
also check architectural and backend errors plus a committed STOP-after-store.
This remains a bounded fixture, not an
`od-machine`/MMU owner integration or a performance claim. See the
[S3-20l report](../reports/STAGE_3_S3-20l_2026-10-27.md).

#### S3-20m. Direct-base signed displacement through shared IR and dispatch

Shared checked singletons additionally admit only `48 8B/89 /r disp8` with
ModRM `mod=01` and exact length four, or `48 8B/89 /r disp32` with ModRM
`mod=10` and exact length seven. Base and data/source register are independently
bounded to RAX..RBX; SIB, `67`, any extra prefix, register form and trailing
bytes remain rejected. The decoder-provided signed `MemoryOperand` remains the
only effective-address source, and the existing checked-memory helper performs
the canonical, overflow and access checks.

Direct V2, shared IR, interpreter, and bounded dispatcher tests cover disp8
extrema and disp32 extrema/wraparound, aliasing load destination/base and store
source/base, noncanonical starts/ends, backend failures, successful STOP after
store, and partial byte windows. Mixed dispatch composes a new disp8 singleton
with the existing NOP fallback and pair. No pair or unsafe data-RAM contract
changes. See the [S3-20m report](../reports/STAGE_3_S3-20m_2026-10-27.md).

#### S3-20n. No-displacement SIB MOV in shared IR and bounded dispatch

Shared checked singleton compilation additionally accepts exactly four-byte
`48 8B/89 /r sib`: ModRM `mod=00,r/m=100`, data/base/index encodings
RAX..RBX, qword operand/address sizes, and SIB scale 1/2/4/8. SIB index
`100` (no index), base `101` (no base), other base/index encodings,
displacement forms, `67`, other prefixes and trailing bytes remain rejected.
The `MemorySiteV2` source validator binds ModRM/SIB to decoded operation,
registers, base/index and scale. IR verification also requires the four-byte
instruction range. Direct-base disp8 remains separately admitted by ModRM
`mod=01`, so its four-byte length cannot be confused with SIB `mod=00`.

The decoder-provided `MemoryOperand` and existing `access_address_48` remain
the only effective-address semantics; no SIB address calculation was added.
Native direct V2/shared-IR/interpreter tests enumerate both operations, all
data/base/index registers and scales, including aliases and a zero-valued
index. SIB-derived cases cover multiply/add wrapping, noncanonical start/end,
backend errors and a committed store followed by STOP. Bounded dispatch
composes SIB singleton → bare NOP interpreter fallback → existing pair across
budgets 0..4 and partial pre-SIB byte windows. Pair admissions and the unsafe
private synchronous RAM contract are unchanged. See the
[S3-20n report](../reports/STAGE_3_S3-20n_2026-10-27.md).

#### S3-20o. Address32 direct-base qword MOV in shared IR and bounded dispatch

The shared checked singleton additionally accepts only exact
`67 48 8B/89 /r`, ModRM `mod=00`, data/base RAX..RBX, instruction length
four. The descriptor retains the architectural distinction between qword
operand width and dword effective-address width. No SIB, displacement,
extended register, reordered/extra prefix, or trailing byte is accepted. The
raw-byte check is a separate branch from four-byte SIB (`48` prefix) and
direct disp8 (`mod=01`) forms. The decoder-provided `MemoryOperand` and
`access_address_48` remain the sole address semantics; no mask, zero-extension,
or qword access splitting was added.

Native direct V2/shared IR/interpreter cases cover load/store and every data
and base register pair, including source/destination aliasing the base and
noncanonical u64 base values whose low dword is used. A qword at
`0xffff_fffc` uses one contiguous callback through `0x1_0000_0003`. Exact
architectural faults preserve CPU/RAM/budget/retirement, backend errors remain
host errors, guards skip access, and a successful store remains committed
when STOP follows. Bounded dispatch composes the singleton → bare NOP
interpreter fallback → existing pair with budgets 0..4, STOP and continuation
from actual RIP. See the
[S3-20o report](../reports/STAGE_3_S3-20o_2026-10-27.md).

#### S3-20p. Address32 signed displacement through shared IR and bounded dispatch

The shared checked singleton additionally admits only exact
`67 48 8B/89 /r disp8|disp32` with ModRM `mod=01/10`, data/base RAX..RBX,
and exact lengths five/eight. The descriptor retains qword data width and
dword address width plus the decoder-provided signed displacement. Existing
`MemoryOperand::access_address_48` remains the only EA and access-range
semantics; admission checks the encoding and displacement range but does not
re-sign-extend or compute an address. No SIB, RIP-relative form,
extra/reordered prefixes, wider register, or multi-instruction profile is
added; pair admissions remain unchanged.

Native direct V2/shared IR/interpreter cases compare disp8 -128/-1/0/1/127
and disp32 MIN/MIN+1/-1/0/1/MAX-1/MAX for load and store, with destination or
source aliasing the base. They verify high base bits are ignored, signed
addition wraps modulo 2^32, and a qword at `0xffff_fffc` uses one contiguous
8-byte callback through `0x1_0000_0003`. IR architectural faults preserve exact
payload and CPU/RAM/budget/retirement; backend failures remain host errors;
successful store+STOP remains committed. Bounded dispatch composes disp8
singleton → bare NOP fallback → existing pair for budgets 0..4 and resumes
from the actual RIP; partial disp8/disp32 windows stop without effects. See
the [S3-20p report](../reports/STAGE_3_S3-20p_2026-10-27.md).

#### S3-20q. 64-bit SIB displacement through shared IR and bounded dispatch

The shared checked singleton additionally admits exactly
`48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`; RAX..RBX data,
base, and index; SIB scale 1/2/4/8; and source lengths five/eight. It rejects
no-index/no-base encodings, `67`, extended registers, and additional prefixes.
The SIB-with-displacement source path is distinct from address32 direct-base
displacements and direct-base 64-bit displacement forms; length alone does
not select an addressing mode. Descriptor displacement and operand fields
come from the decoder, and `MemoryOperand::access_address_48` remains the only
effective-address, canonicality, and full qword-range implementation.

Native direct V2/shared IR/interpreter parity covers disp8 -128/-1/0/127 and
disp32 MIN/-1/0/MAX across all scales and both MOV directions. Additional
cases alias data with base/index and base with index. Callback address and
length checks verify one qword operation, including signed arithmetic that
wraps into the canonical target. Architectural fault payloads preserve CPU,
RAM, budget, and retirement across all three execution paths; backend errors
remain host errors, and SIB store+STOP retains its commit. Bounded dispatch
composes the disp8 singleton → bare NOP fallback → existing pair across
budgets 0..4, resumes from the actual RIP after STOP, and rejects invalid SIB
fields without a helper call. Partial disp8/disp32 windows stop before
publication or data access. Pair admissions remain unchanged. See the
[S3-20q report](../reports/STAGE_3_S3-20q_2026-10-27.md).

#### S3-20r. Address32 no-displacement SIB through shared IR and bounded dispatch

The shared checked singleton admits exactly `67 48 8B/89 /r sib` with ModRM
`mod=00,r/m=100`, RAX..RBX data/base/index registers, scale 1/2/4/8, qword
data, dword address size and exact length five. The raw source validator
distinguishes it from address32 direct-base disp8 (ModRM `mod=01`), ordinary
64-bit SIB (no `67` prefix), and displacement-bearing SIB forms. The decoder's
`MemoryOperand` and `access_address_48` remain the only address semantics; no
EA computation was added. No-index/no-base specials, RSP/RBP or extended
register encodings, address32 SIB with displacement, extra prefixes and
trailing bytes remain unsupported. The 9-byte addr32 SIB disp32 form is
outside the fixed eight-byte site storage and is rejected. Pair admissions
remain unchanged.

Descriptor and native direct V2/shared IR/interpreter checks cover both MOV
directions, every supported data/base/index register and scale, aliases,
ignored high GPR halves, and wrapping modulo 2^32. Callback evidence includes
a qword beginning at `0xffff_fffc`, proving the qword access continues linearly
past 4 GiB. Fault payloads preserve state/RAM/budget/retirement, backend errors
remain host errors, and successful store+STOP retains its commit. Bounded
dispatch composes addr32 SIB → bare NOP fallback → existing pair at budgets
0..4, checks no-effect guards and continuation from actual RIP, and rejects
partial windows and forbidden fields before memory access. See the
[S3-20r report](../reports/STAGE_3_S3-20r_2026-10-27.md).

#### S3-20s. Address32 signed disp8 SIB through shared IR and bounded dispatch

The checked singleton additionally admits exactly `67 48 8B/89 /r sib disp8`,
ModRM `mod=01,r/m=100`, RAX..RBX data/base/index, SIB scale 1/2/4/8, qword
data, dword address size and exact length six. The site stores at most eight
source bytes; address32 SIB disp32 remains rejected because it is nine bytes.
The decoder's `MemoryOperand` and `access_address_48` remain the sole
displacement/effective-address and qword-range semantics. No prefix variants,
special SIB fields, pair changes, or alternate encodings are admitted.

Descriptor and direct V2/shared IR/interpreter cases cover disp8
`-128/-1/0/127`, all scales and supported register aliases, high GPR halves,
modulo-2^32 base/index/displacement arithmetic, and contiguous qword access
across `0xffff_ffff`. Fault payloads and no-commit behavior, backend errors,
and store+STOP commit are checked. Bounded dispatch composes the six-byte
singleton → bare NOP fallback → existing pair across budgets 0..4; a six-byte
singleton boundary proves the form is not mistaken for a two-instruction pair
by length. Continuation resumes at actual RIP without repeating effects.
Partial byte windows and forbidden encodings stop before helper access. See
the [S3-20s report](../reports/STAGE_3_S3-20s_2026-10-27.md).

#### S3-20t. Address32 signed disp32 SIB through shared IR and bounded dispatch

The checked singleton admits exactly `67 48 8B/89 /r sib disp32`, ModRM
`mod=10,r/m=100`, RAX..RBX data/base/index, SIB scale 1/2/4/8, qword data,
dword address size and exact length nine. Singleton source metadata capacity
is now the named `MEMORY_SITE_SOURCE_BYTES_MAX = 9`; the Rust return type of
`source_bytes_storage()` changes from `([u8; 8], u8)` to `([u8; 9], u8)`.
This capacity applies only to singleton source identity. Qword data staging,
two-site source `[u8; 6]`, ActiveCodeV3 source `[u8; 23]`, and RuntimeContext/C
ABI layouts are unchanged. Validate the decoded length before slicing or
copying all nine bytes. The decoder's `MemoryOperand` and
`access_address_48` remain the only displacement and effective-address source.

Descriptor and direct V2/shared IR/interpreter tests cover disp32 MIN/-1/0/MAX
for both operations, all scales and the RAX address/data alias, including the
contiguous qword callback at `0xffff_fffc`. A pair of code sources differs only
in byte nine and retains distinct displacement metadata. Architectural fault
payloads preserve CPU/RAM/budget/retirement; backend errors remain host errors;
successful store+STOP retains its write. Bounded dispatch composes the
nine-byte singleton → bare NOP interpreter bridge → existing pair, checks
budgets 0..4, stop-RIP, callback STOP, and continuation from actual RIP without
repeating the first access. Windows 1..8 and malformed encodings are rejected
before a helper callback. See the [S3-20t report](../reports/STAGE_3_S3-20t_2026-10-27.md).

#### S3-20u. RBP-base signed disp8 through shared IR and bounded dispatch

Admit only the singleton `48 8B/89 /r disp8`: ModRM `mod=01,r/m=101`,
RBP base, data register RAX..RBX, qword data, 64-bit effective address and
exactly four source bytes. Decoder `MemoryOperand::access_address_48` remains
the sole displacement and address-range implementation. RSP/SIB, address-size
override, disp32, segment or extra prefixes, extended registers, and pairs are
outside this slice.

Direct V2/shared IR/interpreter checks cover both operations and disp8
`-128/-1/0/127`, exact callback address/count, complete state/RAM parity, and
unchanged RBP/RSP. A noncanonical RBP start and an access whose last byte is
noncanonical both produce #SS (vector 12, error 0, no linear address); the
same checks with RBX produce #GP. Faults make no data callback and preserve
state/budget/retirement. AddressOverflow remains its host diagnostic, backend
errors remain host errors, entry STOP/budget zero make no access, and a store
that requests STOP after writing remains committed. Bounded dispatch composes
the singleton → bare NOP interpreter bridge → existing pair, matches
interpreter for budgets 0..4, resumes from the actual RIP without replay, and
keeps RBP/RSP unchanged. See the [S3-20u report](../reports/STAGE_3_S3-20u_2026-10-27.md).

#### S3-20v. RSP-base no-index SIB through shared IR and bounded dispatch

Admit only singleton `48 8B/89 /r sib` with ModRM `mod=00,r/m=100` and exact
SIB byte `0x24`: RSP base, no index, scale 1, no displacement, data RAX..RBX,
qword data, 64-bit effective address and exact four-byte length. The decoder's
`MemoryOperand` and `access_address_48` remain the sole address semantics; host
SP and frame ABI are unchanged. Other SIB fields, displacement modes, RSP as a
data register, `67`, other prefixes, extended registers, and pair changes are
excluded.

Direct V2/shared IR/interpreter load/store parity checks compare full state,
RAM, callback address/count and retirement, including unchanged RSP/RBP/flags.
Noncanonical RSP start and last-byte ranges produce #SS; identical RBX controls
produce #GP, before data access and with no architectural commit. AddressOverflow
and backend failures remain distinct host errors; entry STOP/zero budget do not
access memory and store+STOP retains its write. Bounded dispatch exercises the
four-byte singleton → bare NOP → existing pair, budgets 0..4, actual-RIP
continuation and partial byte windows. Invalid near-matches return Unsupported
without publishing a block or calling memory. See the
[S3-20v report](../reports/STAGE_3_S3-20v_2026-10-27.md).

#### S3-20w. RSP-base no-index SIB with signed disp8

Extend only the singleton forms `48 8B/89 /r sib disp8`, ModRM
`mod=01,r/m=100`, exact SIB `0x24`, data RAX..RBX, qword data and 64-bit EA.
The decoded `MemoryOperand` and `access_address_48` remain the sole source of
signed displacement and effective-address semantics. No disp32, indexed RSP,
address-size/segment/extra prefix, extended register, pair/V3, or emitter change
is included; the existing `[RSP]` form remains separately admitted.

Evidence covers disp8 -128/-1/0/127 for load/store across direct V2, shared IR
and interpreter, including callback address/count and full architectural/RAM
parity. RSP/RBP/flags stay unchanged. The shared fault matrix retains RSP #SS
versus RBX #GP, AddressOverflow versus backend errors, no-commit faults and
committed store+STOP. Bounded dispatch composes the five-byte singleton → bare
NOP interpreter bridge → existing pair across budgets 0..4, STOP-RIP and
actual-RIP continuation without replay; truncated windows and invalid shapes
have no memory effects. See the
[S3-20w report](../reports/STAGE_3_S3-20w_2026-10-27.md).

#### S3-20x. RSP-base no-index SIB with signed disp32

Extend only singleton `48 8B/89 /r sib disp32` with ModRM
`mod=10,r/m=100`, exact SIB `0x24`, RSP base/no index, data RAX..RBX,
qword data, 64-bit EA and length eight. The four displacement bytes begin
after the SIB; decoded `MemoryOperand` and `access_address_48` own signed
interpretation and address calculation. The source metadata capacity already
holds the eight-byte form; no data-buffer, pair, V3, or ABI change is needed.
Address32, other prefixes, indexed/special SIB forms, pair/V3 changes and
additional emitters remain excluded. Existing `[RSP]` and RSP+disp8 forms stay
separately covered.

Direct V2/shared IR/interpreter tests cover both operations, disp32
MIN/-1/0/MAX, contiguous qword access across 4 GiB and signed address wrap to
zero. Sources differing only in the high displacement byte produce distinct
expected callback targets. Fault tests check RSP #SS versus RBX #GP, diagnostic
AddressOverflow, backend error, no commit and successful store+STOP. Bounded
dispatch composes the eight-byte singleton → NOP → pair across budgets 0..4,
STOP and actual-RIP continuation; partial windows 1..7 and invalid forms make
no effects. See the
[S3-20x report](../reports/STAGE_3_S3-20x_2026-10-27.md).

#### S3-20y. RBP-base signed disp32 qword singleton

Extend only `48 8B/89 /r disp32`, ModRM `mod=10,r/m=101`, RBP base,
data RAX..RBX, qword data, 64-bit EA and exact length seven. The decoder's
`MemoryOperand` and `access_address_48` remain the sole source of signed
displacement and effective-address semantics. Keep `mod=00,r/m=101` as the
existing instruction-local RIP-relative form; no SIB, address override,
other prefix/base or pair/V3 changes are included.

Direct V2/shared IR/interpreter evidence covers both directions, disp32
MIN/-1/0/127/128/MAX, callback address/count and architectural/RAM parity.
The same displacement through RBP and RIP-relative encoding must resolve to
their distinct expected targets. Preserve RBP #SS versus RBX #GP at the
noncanonical start/end, diagnostic AddressOverflow and backend error
classification, fault atomicity, STOP/budget guards, and committed store+STOP.
Bounded dispatch composes RBP disp32 → bare NOP → existing pair across budgets
0..4, STOP-RIP and actual-RIP continuation without replay; windows 1..6 make
no effects. See the [S3-20y report](../reports/STAGE_3_S3-20y_2026-10-27.md).

#### S3-20ab. RBP as both qword MOV data and disp8 base

Admit only exact singleton `48 8B/89 6D ib`: ModRM `mod=01`, `reg=RBP`,
`r/m=RBP`, signed disp8, qword data, 64-bit effective address and four source
bytes. The decoded `MemoryOperand` and `access_address_48` remain the only
displacement/address implementation. No disp32, SIB, address-size or other
prefix, other data/base combinations, pairs, or frame/ABI changes are included;
existing RBP-data-only, RBP-base-only and RSP-data profiles remain supported.

Direct V2/shared IR/interpreter tests cover load/store at disp8 -128/-1/0/127.
The effective address and store payload use entry RBP before destination
commit; a load replaces RBP with a different value. A noncanonical current
access faults #SS before callback and leaves state/RAM/budget/retirement
unchanged. Bounded dispatch splits after a successful RBP-relative load,
resumes at the actual next RIP without repeating the read, and faults on a
subsequent RBP-based access while retaining the first commit. An ARM64/macOS
x19 ABI canary covers this alias profile's success, budget-guard and fault
returns; frame layout is unchanged. See the
[S3-20ab report](../reports/STAGE_3_S3-20ab_2026-10-27.md).

#### S3-20ac. RSP as both qword MOV data and no-index SIB base

Admit only exact singleton `48 8B/89 24 24`: ModRM `mod=00/rm=100`, data
register RSP, SIB exactly `0x24`, RSP base/no index/scale 1, no displacement,
qword data, 64-bit effective address and exact length four. The decoder's
`MemoryOperand` and `access_address_48` remain the sole address authority.
Other SIB values, displacement, index, prefix, data/base combinations, pairs,
and frame/ABI changes remain excluded; prior RSP data-only and RSP-base
RAX..RBX forms remain supported.

Direct V2/shared IR/interpreter checks compare full state/RAM, callback
address/count and retirement for load/store. Address and store payload use
entry RSP; a successful load commits a distinct new value. The current
noncanonical `[RSP]` access faults #SS before a data callback and commits
nothing. An ARM64/macOS x19 canary covers this exact profile's success, budget
guard and fault returns. Bounded dispatch pauses after the load on both budget
and callback STOP, resumes at the actual next RIP, then faults on a following
`[RSP]` access without repeating the first read. Invalid SIB and partial forms
stop before publication/helper calls. See the
[S3-20ac report](../reports/STAGE_3_S3-20ac_2026-10-27.md).

#### S3-20ad. RSP as both qword MOV data and signed-disp8 SIB base

Admit only exact singleton `48 8B/89 64 24 ib`: ModRM `mod=01/reg=4/rm=4`,
SIB exactly `0x24` (RSP base, no index, scale 1), signed disp8, qword data,
64-bit effective address and exact length five. The existing decoded
`MemoryOperand` and `access_address_48` remain the only address authority.
Generic GPR validation, frame/ABI, pair/V3 and other prefix/address forms do
not change.

Direct V2/shared IR/interpreter parity covers disp8 -128, -1, 0 and 127 for
load/store. Effective address and store payload use entry RSP; successful load
commits the loaded RSP only after the access succeeds. Fault, address-overflow,
backend-error, STOP and zero-budget cases preserve their existing
classification and commit rules. Dispatcher budget split and callback STOP
commit `MOV RSP,[RSP-1]`, resume at `ENTRY+5`, then fault #SS on the following
`[RSP]` access without repeating the first read. Partial SIB, disp32, changed
SIB and prefix variants stop without a data callback. See the
[S3-20ad report](../reports/STAGE_3_S3-20ad_2026-10-27.md).

#### S3-20ae. RSP as both qword MOV data and signed-disp32 SIB base

Admit only exact eight-byte singleton `48 8B/89 A4 24 id`: ModRM
`mod=10/reg=4/rm=4`, SIB exactly `0x24` (RSP base, no index, scale 1),
signed disp32 in source bytes 4..7, qword data and 64-bit effective address.
The existing decoder `MemoryOperand` and `access_address_48` remain the only
address authority; generic GPR validation, frame/storage/ABI, pair and V3
profiles remain unchanged.

Direct V2/shared IR/interpreter parity covers disp32 MIN/-1/0/MAX. Entry RSP
plus signed displacement is checked against the target, and load commits a
distinct new RSP only after success; store writes the full old RSP. Two sources
that differ only in byte 7 produce the independently expected callback targets
`0x01000040` and `0x02000040`. Fault and backend errors retain their existing
classification, current #SS has no data callback, and qword range overflow is
not converted to a guest fault. Budget/STOP split and exact store+STOP preserve
commit/accounting. Dispatcher composes this singleton with NOP and the existing
pair, resumes at actual RIP without replay, and rejects partial windows 1..7
and malformed SIB/prefix forms before effects. See the
[S3-20ae report](../reports/STAGE_3_S3-20ae_2026-10-27.md).

#### S3-20af. RBP as both qword MOV data and signed-disp32 base

Admit only exact seven-byte singleton `48 8B/89 AD id`: ModRM
`mod=10/reg=5/rm=5`, signed disp32 bytes 3..6, qword data and 64-bit EA.
The decoder's `MemoryOperand` and `access_address_48` remain the sole address
authority. This exact form does not widen generic register predicates; the
mod00/rm5 RIP-relative form remains separate and uses this instruction's
NextRip. Direct V2/shared IR/interpreter parity covers disp32 MIN/-1/0/MAX,
full CPU/RAM state, callback addresses/counts and retirement. The old RBP is
used as address and store data; a successful load commits the result into RBP.
Byte-six identity and the RBP-vs-RIP address distinction are explicit.
Partial windows 1..6 and malformed forms reject. Bounded continuation commits
the first RBP load, then faults #SS on the next RBP-relative access without
repeating that read. Existing data-only/base-only/RSP profiles remain covered.
See the [S3-20af report](../reports/STAGE_3_S3-20af_2026-10-27.md).

#### S3-20ag. RBP data with the exact RSP-base SIB

Admit only four-byte singleton `48 8B/89 2C 24`: ModRM
`mod=00/reg=5/rm=4`, SIB exactly `0x24` (RSP base, no index, scale 1, no
displacement), qword data and 64-bit EA. The decoded `MemoryOperand` remains
the address authority. This exact exception does not widen generic register
predicates; other RBP-data SIB forms, prefixes and displacement forms remain
unsupported.

Direct V2/shared IR/interpreter parity covers load/store with distinct RSP and
RBP values. The old RSP supplies the effective address and store leaves RBP
unchanged; load updates RBP only after a successful read. Noncanonical RSP
produces #SS before callback, while noncanonical RBP is accepted as store data
at canonical RSP. A successful store callback STOP preserves all eight bytes.
Dispatcher continuation after `MOV RBP,[RSP]` resumes at actual RIP and uses
the newly loaded RBP for the next instruction without replaying the first read.
See the [S3-20ag report](../reports/STAGE_3_S3-20ag_2026-10-27.md).

#### S3-20ah. RBP data with RSP-base SIB displacement

Add only exact qword singleton forms `48 8B/89 6C 24 ib` (signed disp8,
length 5) and `48 8B/89 AC 24 id` (signed disp32, length 8): ModRM
`mod=01/10`, `reg=5`, `rm=4`, SIB exactly `0x24` (RSP base, no index,
scale 1). The guest RSP supplies the effective-address base and RBP supplies
the load destination/store value. Use the decoder's `MemoryOperand` for all
address semantics; do not widen generic GPR predicates or change pair/V3,
source storage, frame layout or ABI.

Direct V2/shared IR/interpreter checks cover both widths, signed boundaries,
complete state/RAM and callback/retirement parity. They also verify #SS on
noncanonical RSP without a callback, noncanonical RBP as store payload,
address-overflow/backend error classification, and store+STOP commit. The
bounded dispatcher checks both widths in singleton→NOP→pair continuation and
uses a loaded RBP for the next RBP-relative access. Partial windows and
near-match forms stop before side effects. See the
[S3-20ah report](../reports/STAGE_3_S3-20ah_2026-10-27.md).

#### S3-20ai. RSP data with an RBP-base direct displacement

Add only singleton `48 8B/89 65 ib` (signed disp8, length 4) and
`48 8B/89 A5 id` (signed disp32, length 7): ModRM `mod=01/10`,
`reg=4` (RSP data), `rm=5` (RBP base), qword data and 64-bit EA. Do not
admit SIB, prefixes, other register combinations, pairs or V3. Decoded
`MemoryOperand` remains the only EA semantics; generic register validation,
frame and ABI do not change. Keep `mod=00/rm=5` RIP-relative and existing
RBP-data `mod=10/rm=5` profiles distinct.

Direct V2/shared IR/interpreter acceptance covers both widths and signed
boundaries, including disp32 source byte-six identity, independent
`RBP+displacement` target formulas, RSP/RBP alias semantics and full state/RAM
parity. Noncanonical RSP is valid data; a noncanonical RBP address raises #SS
before callback. Store+STOP keeps all old RSP bits. Dispatch resumes from
actual RIP and uses a successfully loaded RSP in the next `[RSP]` access;
budget splits and STOP do not repeat the first effect. Partial and forbidden
forms have no effects. See the
[S3-20ai report](../reports/STAGE_3_S3-20ai_2026-10-27.md).

#### S3-20aj. RSP/RBP data with a low-GPR direct displacement

Add singleton `48 8B/89 /r disp8` (length 4) and
`48 8B/89 /r disp32` (length 7), with ModRM `mod=01/10`, data field RSP or
RBP, and direct base RAX..RBX. SIB, prefixes, other data/base registers, pairs
and V3 remain outside this slice. Effective-address semantics continue to
come from the decoded `MemoryOperand`; generic guest-register admission is not
widened.

Direct V2, shared IR and interpreter parity cover all four bases, both data
registers and both widths. Noncanonical data remains valid until used as an
address; noncanonical low-GPR effective addresses raise #GP before callback.
Stores commit before callback STOP. Bounded dispatch covers actual-RIP budget
and STOP continuation, singleton→NOP→pair composition, and later #SS when a
loaded noncanonical RSP/RBP is used as the next memory base. Partial byte
windows and unsupported near matches stop without effects. The existing
RBP-base and RIP-relative profiles remain separate. See the
[S3-20aj report](../reports/STAGE_3_S3-20aj_2026-10-27.md).

#### S3-20ak. RIP-relative qword MOV with RSP/RBP data

Extend the exact singleton RIP-relative `48 8B/89 /r disp32` profile to
`reg=RSP|RBP` only (`48 8B/89 25/2D disp32`, seven bytes). Preserve the
decoded `MemoryOperand::NextRip` as the sole effective-address authority;
do not widen any generic register predicate. Prefixes, SIB, pairs and V3 stay
outside this slice.

Direct V2/shared-IR/interpreter host parity verifies both operations at a
CodeView interior entry and uses an independent entry `NextRip + disp32`
target. Noncanonical RIP-relative targets are exact #GP faults before a RAM
callback; a noncanonical value loaded into RSP/RBP is data and commits, while
using it as the next RSP/RBP address raises #SS without repeating the first
read. Bounded dispatch exercises NOP→RSP/RBP RIP-relative singleton→two
following singleton memory operations, budgets 0..4, and continuation from
the singleton's actual next RIP. See the
[S3-20ak report](../reports/STAGE_3_S3-20ak_2026-10-27.md).

#### S3-20al. RSP/RBP data through indexed 64-bit SIB

Only the current 64-bit indexed-SIB singleton subset is extended: no
displacement (`mod=00`), signed disp8 (`mod=01`) and signed disp32 (`mod=10`),
with data register RSP or RBP, base/index RAX..RBX and scale 1/2/4/8. The
existing decoder `MemoryOperand` remains the source of effective-address
semantics. Direct V2, shared IR and the bounded dispatcher must admit the same
exact profile; no pair, V3, address-size override or special SIB shape is
included. Evidence and remaining host-owner limits are in the
[S3-20al report](../reports/STAGE_3_S3-20al_2026-10-27.md).

#### S3-20am. RSP/RBP data through address32 direct-base qword MOV

This slice adds only direct address32 forms with qword data and RSP or RBP in
the ModRM data field. The memory base is RAX..RBX, there is no SIB or RIP
relative form, and the exact instruction lengths are 4, 5 and 8 bytes for
no-displacement, signed disp8 and signed disp32. Decoder metadata remains the
sole authority for address-size truncation and signed displacement. Tests must
prove the low-32-bit effective address independently, preserve all 64 data
bits, and treat a qword access crossing 4 GiB as one contiguous helper call.
See [S3-20am evidence](../reports/STAGE_3_S3-20am_2026-10-27.md).

#### S3-20an. RSP/RBP data through address32 indexed-SIB qword MOV

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]` forms with ModRM
`mod=00/01/10,r/m=100`, data RSP or RBP, and low-GPR base/index RAX..RBX.
SIB scales remain 1/2/4/8; special no-index/no-base SIB encodings and RSP/RBP
base/index are excluded. Exact lengths are 5/6/9 bytes for no displacement,
signed disp8 and signed disp32. Address size is Dword and operand size Qword;
the existing decoded `MemoryOperand` remains the only authority for EA
calculation. Keep direct V2, shared IR and dispatch admission aligned without
changing pairs or V3. Tests cover address32 wrapping, contiguous qword access,
stack-data commit/fault semantics, the ninth displacement byte, and dispatch
continuation. See [S3-20an evidence](../reports/STAGE_3_S3-20an_2026-10-27.md).

#### S3-20ao. RSP/RBP data through address32 no-index SIB

Admit only singleton `67 48 8B/89 /r 24 [disp8|disp32]` forms with ModRM
`mod=00/01/10,r/m=100` and SIB exactly `0x24`: guest RSP is the old
low-32-bit base, there is no index, scale is one, and the data field is one
of RAX..RBP. Address size is Dword and transfer size remains Qword; lengths
are 5/6/9 bytes. The decoder's `MemoryOperand` and
`access_address_48` remain the only effective-address semantics. No other SIB,
address-size, pair or V3 form is admitted.

Direct V2, shared IR, and the bounded dispatcher must preserve full-width
data while truncating only the old RSP used for address calculation. Test a
contiguous qword access crossing 4 GiB, signed displacement boundaries,
register aliases including RSP data, callback commit/fault behavior, guard
ordering, and continuation without repeating effects. See the
[S3-20ao report](../reports/STAGE_3_S3-20ao_2026-10-27.md).

#### S3-20ap. RSP/RBP data through address32 direct EBP-base

Admit only `67 48 8B/89 /r disp8|disp32` singleton forms with ModRM
`mod=01/10,r/m=101`. The effective-address base is old guest RBP's low 32
bits, data may be any register RAX..RBP, address size is Dword, and transfer
size remains Qword. Exact lengths are 5 and 8 bytes. `mod=00/rm=101` remains
excluded and is not reclassified as this EBP-based form; SIB, extra prefixes,
pairs and V3 remain outside scope. `MemoryOperand` and
`access_address_48` continue to supply the only EA semantics.

Direct V2, shared IR and dispatch must agree on signed displacement, low-32-bit
wrap, full-width data, and old-RBP alias behavior. Verify the qword crossing
4 GiB is one access, faults do not commit, and a loaded noncanonical RBP can
retire before a later 64-bit RBP-based access faults. See the
[S3-20ap report](../reports/STAGE_3_S3-20ap_2026-10-27.md).

#### S3-20aq. Stack data through indexed SIB with RSP base

Admit only 64-bit-address-size singleton `48 8B/89 /r sib [disp8|disp32]`
forms with ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RAX..RBX and
scale 1/2/4/8. Data may be RAX..RBP and remains a full qword. Exact lengths
are 4/5/8 bytes. The existing no-index SIB `0x24` profile remains separate;
67-prefixed, other base/index, pair and V3 forms remain outside scope.

Use the decoded `MemoryOperand` and `access_address_48` for EA behavior.
Verify old-value aliases for data=RSP/index, direct/shared-IR/interpreter
parity at signed displacement boundaries, #SS on noncanonical RSP start/end,
backend-fault atomicity, STOP/budget continuation and dispatcher resumption
without repeating the first memory access. See the
[S3-20aq report](../reports/STAGE_3_S3-20aq_2026-10-27.md).

#### S3-20ar. Stack data through indexed SIB with RBP base

Admit only 64-bit-address-size singleton `48 8B/89 /r sib disp8|disp32`
forms with ModRM `mod=01/10,r/m=100`, SIB base RBP, index RAX..RBX and
scale 1/2/4/8. Data may be RAX..RBP and remains qword. Exact lengths are 5/8
bytes. The `mod=00,SIB.base=101` encoding means no base and remains rejected;
the existing RSP-base indexed and no-index SIB profiles remain unchanged.

Use the decoded `MemoryOperand` and `access_address_48` for EA behavior.
Verify old-value aliases for data=RBP/index, signed displacement edges and
wrapping, #SS for noncanonical RBP independent of data/index, #GP through a
low-GPR control base, address/backend error atomicity, and STOP/budget
continuation without repeating effects. See the
[S3-20ar report](../reports/STAGE_3_S3-20ar_2026-10-27.md).

#### S3-20as. Address32 indexed SIB with RSP base

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]` forms with
ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RAX..RBX and scale
1/2/4/8. Data may be RAX..RBP; address calculation is Dword and transfer is
Qword. Exact lengths are 5/6/9 bytes. No-index `SIB=0x24` remains its existing
separate profile; no-base and RSP/RBP index encodings, extra prefixes, pairs
and V3 remain unsupported.

The decoder's `MemoryOperand` and `access_address_48` supply all EA semantics.
Verify upper-half truncation, signed modulo-2^32 addition, old-state base/index
and data aliases, a contiguous qword crossing 4 GiB, direct V2/shared IR/
interpreter parity, fault atomicity, committed store+STOP, and actual-RIP
dispatcher continuation. See the
[S3-20as report](../reports/STAGE_3_S3-20as_2026-10-27.md).

#### S3-20at. Address32 indexed SIB with RBP base

This slice admits singleton qword MOV forms `67 48 8B/89 /r sib disp8|disp32` with ModRM `mod=01/10,r/m=100`, SIB base RBP, an RAX..RBX index, scale 1/2/4/8, and data RAX..RBP. Address calculation uses Dword semantics while the transfer remains Qword; instruction lengths are exactly six or nine bytes. The existing decoded `MemoryOperand` and `access_address_48` remain the effective-address source. `mod=00,SIB.base=101` is the no-base form and remains excluded; no-index SIB, extra prefixes, pairs and V3 are unchanged.

The focused matrices cover source/decoded-profile validation, direct V2/shared IR/interpreter parity, full qword callbacks across the 4-GiB boundary, old-state data/base/index aliases, architectural and backend error atomicity, STOP after committed stores, dispatcher budget continuation from actual RIP, and partial-window/no-base rejection. Evidence and exact commands are in the [S3-20at report](../reports/STAGE_3_S3-20at_2026-10-27.md). Owner-backed RAM integration, physical x86, Mini and performance evidence remain open.

#### S3-20au. Absolute qword MOV через no-base/no-index SIB

Принимать только singleton `48 8B/89 /r 25 disp32`: ModRM `mod=00,r/m=100`, SIB ровно `0x25` (scale 1, index none, base none), data RAX..RBP, длина 8. EA — sign-extended disp32 в 64-битном адресном размере; decoder `MemoryOperand` и `access_address_48` остаются единственным источником семантики. Indexed no-base SIB, `67`, другие SIB, moffs, пары и V3 не добавляются.

Свидетельства включают direct V2/shared IR/interpreter parity для load/store и всех шести data-регистров, независимые абсолютные target значения при изменённых GPR/RIP, полный qword у `-8` до `u64::MAX`, overflow у `-1` до helper, архитектурные/backend ошибки без commit, store+STOP, значимость восьмого source byte и отличие от RIP-relative. Borrowed dispatcher проверяется на бюджете и продолжении с actual RIP; частичные окна 1..7 дают BoundaryReached без эффектов, полный addr32 neighbor — Unsupported. Результаты и границы: [S3-20au report](../reports/STAGE_3_S3-20au_2026-10-27.md). Production owner/MMU integration, physical x86, Mini и performance остаются открытыми.

#### S3-20av. Address32 absolute qword MOV через no-base/no-index SIB

Принимать только singleton `67 48 8B/89 /r 25 disp32`: ModRM `mod=00,r/m=100`, SIB ровно `0x25`, data RAX..RBP, длина 9. `MemoryOperand` и `access_address_48` вычисляют dword-адрес: signed disp32 складывается по модулю 2^32 и zero-extends в 64-битный адрес; qword access затем остаётся единым непрерывным диапазоном и не заворачивается побайтно через 4 GiB. Runtime matcher проверяет профиль bytes opcode/ModRM/SIB и использует displacement из immutable decoder-built artifact, не пере-декодируя raw disp32. 64-bit absolute 20au сохранён; indexed no-base, прямой `rm=101`, moffs, дополнительные префиксы, пары и V3 не добавляются.

Приёмка покрывает direct V2/shared IR/interpreter для обоих направлений и всех шести data-регистров, disp32 границы и независимый low-32-bit target, qword через `0xffff_ffff`, faults/backend без commit, store+STOP, только девятый байт source в независимых artifacts, dispatcher budget/resume без replay и partial windows 1..8. Отчёт: [S3-20av report](../reports/STAGE_3_S3-20av_2026-10-27.md); owner/MMU, physical x86, Mini и performance остаются отдельными открытыми gates.

#### S3-20aw. Address64 indexed no-base SIB disp32 qword MOV

Принимать только singleton `48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длина 8. No-index SIB `0x25` 20au остаётся отдельной формой. `MemoryOperand` и `access_address_48` задают полный 64-битный EA `index*scale + sign_extended_disp32` с адресной арифметикой по модулю `2^64`; проверка canonical/range и один непрерывный qword callback выполняются тем же helper. Runtime matcher проверяет фиксированный source profile и сравнивает decoded index/scale, используя displacement из immutable decoder-built artifact без повторного decode raw displacement.

Приёмка: direct V2/shared IR/interpreter сравниваются на обоих операциях, всех шести data-регистрах, четырёх index-регистрах, четырёх scale и границах disp32. Проверяются alias `data=index` из pre-state, EA без вклада base/RIP (`index*scale + signed disp32`), u64 scale/add wrap, high displacement byte identity, #GP для noncanonical computed address без callback даже с RSP/RBP data, AddressOverflow для qword end, faults/backend без commit, store+STOP и dispatcher budget/resume без replay. Partial windows 1..7 и недопустимые соседние SIB формы не публикуют блок и не вызывают memory helper. См. [S3-20aw report](../reports/STAGE_3_S3-20aw_2026-10-27.md); production owner/MMU integration, physical x86, Mini и performance остаются открытыми.

#### S3-20ax. Address32 indexed no-base SIB disp32 qword MOV

Принимать только singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длина ровно 9. Это Dword EA и Qword transfer; 64-bit address-size S3-20aw и no-index SIB `0x25` S3-20av остаются отдельными формами. Декодированный `MemoryOperand` и `access_address_48` единолично вычисляют `(low32(index) * scale + disp32) mod 2^32`, затем zero-extend; qword access остаётся непрерывным через границу 4 GiB. Runtime связывает opcode/ModRM/SIB с immutable decoder artifact, не пере-декодируя displacement bytes.

Приёмка сравнивает direct V2/shared IR/interpreter на обоих направлениях, шести data-регистрах, четырёх index-регистрах, scale 1/2/4/8 и disp32 MIN/-1/0/1/MAX. Проверяются независимая формула low32 EA, высокие биты index, `data=index` из pre-state, полный qword payload, значимость девятого байта, contiguous callback у `0xffff_ffff`, fault/backend no-commit, store+STOP и dispatcher actual-RIP continuation без replay. Partial windows 1..8 и unsupported SIB сосед автоматически effects не выполняют. Отчёт: [S3-20ax report](../reports/STAGE_3_S3-20ax_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, физическое исполнение и performance остаются вне этого среза.

#### S3-20ay. Address64 indexed SIB без displacement с индексом RBP

Добавить отдельную exact ветку только для singleton `48 8B/89 /r sib`: ModRM `mod=00,r/m=100`, data RAX..RBP, base RAX..RBX, index RBP, scale 1/2/4/8, ровно 4 bytes. Не расширять общий whitelist индексов: индексное поле `100` при `REX.X=0` остаётся кодировкой no-index; не принимать RSP/RBP/no-base/RSI/RDI как base или индекс в этой новой ветке. Использовать decoder `MemoryOperand` и `access_address_48`; EA — `base + RBP*scale` с wrapping u64.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store ×6 data ×4 base ×4 scale, с отдельной формулой target, high bits RBP, alias `data=RBP`/`data=base` из pre-state, полным qword store, wrap, #GP для start/end без callback, AddressOverflow и backend-classification. Успешный store+STOP остаётся committed один раз. Bounded dispatch проверяет split бюджета и возобновление по фактическому RIP без повторного singleton access; частичные окна и недопустимые соседние SIB формы не выполняют эффекты. Сохранить no-index `SIB=0x24` и no-base `SIB=0x2d` проверки. См. [S3-20ay report](../reports/STAGE_3_S3-20ay_2026-10-27.md); owner/MMU integration, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20az. Address64 indexed SIB disp8/disp32 с индексом RBP

Добавить exact singleton-профиль `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, data RAX..RBP, base RAX..RBX, index RBP, scale 1/2/4/8, длина 5/8 bytes. Не расширять общий whitelist индексов; профиль без displacement S3-20ay остаётся отдельным. Запрещены address-size override, stack/no-base base, другие index, pair и V3. `MemoryOperand` и `access_address_48` остаются единственным источником EA semantics: `base + full64(RBP)*scale + signed displacement` с wrapping u64.

Приёмка сравнивает direct V2/shared IR/interpreter для обоих операций, всех шести data-регистров, четырёх low bases, scales и signed disp8/disp32 границ. Проверяются независимая формула EA, высокие биты RBP, алиасы `data=RBP`/`data=base` из pre-state, qword callback и retirement. Noncanonical start/end должны дать #GP до callback; qword endpoint overflow и backend error сохраняют отдельную классификацию без commit. Store+STOP остаётся committed один раз. Runtime должен показать влияние последнего byte disp32, dispatcher — split и continuation по фактическому RIP без replay, а partial windows/invalid neighbors не дают effects. См. [S3-20az report](../reports/STAGE_3_S3-20az_2026-10-27.md). Production owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20ba. Address32 indexed SIB с индексом RBP

Добавить exact singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, base RAX..RBX, индекс ровно RBP, scale 1/2/4/8, data RAX..RBP; длины 5/6/9, Dword EA и Qword transfer. Существующие 20ay/20az address64 profiles и address32 SIB profiles остаются отдельными; не расширять общий index whitelist и не принимать stack/no-base base, другие индексы, дополнительные prefixes, пары или V3. Decoder `MemoryOperand`/`access_address_48` остаётся единственным источником low32 base/index/displacement семантики.

Приёмка покрывает load/store ×6 data ×4 base ×4 scale и no-disp/disp8/disp32 с signed краями; сравнивает direct V2/shared IR/interpreter полное state/RAM/callback/retirement. Проверяются independent `(low32(base) + low32(RBP)*scale + signed displacement) mod 2^32`, high halves, aliases из pre-state, qword callback через 4 GiB, byte-nine effect, architectural/backend no-commit, store+STOP, partial windows и invalid соседние формы. Bounded dispatcher должен продолжить с фактического RIP после budget split без повторного эффекта. Отчёт: [S3-20ba report](../reports/STAGE_3_S3-20ba_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance не входят в этот срез.

#### S3-20bb. Address64 indexed SIB с RSP base и RBP index

Добавить exact singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base=RSP/index=RBP, scale 1/2/4/8, data RAX..RBP; длины 4/5/8. Общий whitelist индексов не расширяется: добавляется только точная пара RSP base + RBP index, отдельно от ранее принятых RSP-base/low-index и low-base/RBP-index профилей. Нет `67`, RBP/no-base base, других новых индексов, дополнительных prefix, pair или V3. `MemoryOperand` и `access_address_48` остаются единственным источником EA semantics и wrapping u64.

Приёмка покрывает load/store ×6 data ×4 scale × no-disp/disp8/disp32 signed границы с direct V2/shared IR/interpreter parity и independent formula `RSP + RBP*scale + signed displacement mod 2^64`. Host evidence включает aliases из pre-state, u64 wrap, #SS для noncanonical qword начала/конца по RSP base без callback, #GP control по low-GPR base, AddressOverflow отдельно от guest fault, backend no-commit с exact callback, store+STOP commit, dispatcher NOP/pair continuation и partial windows без effects. Отчёт: [S3-20bb report](../reports/STAGE_3_S3-20bb_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance вне этого среза.

#### S3-20bc. Address64 indexed SIB с RBP base и RBP index

Добавить только exact singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP, qword EA/data, lengths 5/8. `mod=00` с SIB base `101` означает no-base и остаётся вне этого профиля. Общий index whitelist не расширяется; pair, V3, `67` и другие новые base/index combinations не принимаются. Decoder `MemoryOperand` и `access_address_48` остаются единственным источником effective-address semantics.

Приёмка покрывает load/store ×6 data ×4 scale × disp8/disp32 signed края; direct V2/shared IR/interpreter parity сравнивает CPU/RAM/callback/retirement и независимую формулу `RBP + RBP*scale + signed displacement mod 2^64`, включая alias `data=base=index=RBP` из pre-state. Noncanonical start/end дают точный #SS без callback, low-base control даёт #GP; qword endpoint overflow остаётся AddressOverflow. Backend failures сохраняют host classification, callback target и no-commit; успешный store+STOP остаётся committed. Dispatcher продолжает после budget split с фактического RIP через NOP и существующую pair без повтора эффекта; partial/invalid формы не публикуются. Отчёт: [S3-20bc report](../reports/STAGE_3_S3-20bc_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance вне этого среза.

#### S3-20bd. Address32 indexed SIB с RSP base и RBP index

Добавить exact singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base=RSP, index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer и длины 5/6/9. Это отдельная точная комбинация; общий индексный whitelist не расширять. Существующие address32 no-index SIB, RSP-base/low-index, low-base/RBP-index и address64 RSP/RBP-index профили остаются самостоятельными. Не добавлять RBP base, no-base, другие индексы, дополнительные prefixes, пары или V3. Decoder `MemoryOperand`/`access_address_48` остаётся единственным источником адресной семантики.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, всех шести data GPR, четырёх scale и mod00/disp8/disp32 signed edges; адрес проверяется независимо как `(low32(RSP) + low32(RBP)*scale + signed displacement) mod 2^32`. Проверяются aliases из pre-state, high-half truncation, qword через 4 GiB единым callback, exact ninth-byte target identity, backend no-commit, store+STOP, partial windows 1..8 и unsupported neighbors. Dispatcher делает budget split после singleton и resumes с actual RIP через NOP и frozen pair без повтора первого memory effect. Отчёт: [S3-20bd report](../reports/STAGE_3_S3-20bd_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20be. Address32 indexed SIB с RBP base и RBP index

Добавить только singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer и точные длины 6/9. `mod=00` с SIB base `101` означает no-base и остаётся запрещённым для этого профиля. Общий индексный whitelist не расширяется; no-displacement, другие base/index combinations, дополнительные prefixes, пары и V3 не добавляются. Decoder `MemoryOperand` и `access_address_48` остаются единственными источниками EA semantics.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, шести data GPR, четырёх scale и signed disp8/disp32 границ с независимой формулой `(low32(RBP) + low32(RBP)*scale + signed displacement) mod 2^32`. Включены pre-state alias `data=base=index=RBP`, игнорирование верхних половин RBP, полные qword payloads и единый callback через границу 4 GiB. Проверяются guest/backend faults без commit, callback STOP после committed store, budget split/resume без повтора, разные targets при изменении source byte 9, partial windows 1..5/1..8 и отклонение no-base neighbor. Для Dword EA старшие биты base не дают #SS: адрес усекается до проверки canonicality. Отчёт: [S3-20be report](../reports/STAGE_3_S3-20be_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance вне среза.

#### S3-20bf. Address64 indexed no-base SIB с RBP index

Принимать только singleton `48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB `base=101` (нет base), `index=RBP`, scale 1/2/4/8, data RAX..RBP, точная длина 8. EA равен `RBP*scale + signed disp32` по модулю 2^64; нет вклада base или RIP. `MemoryOperand::access_address_48` остаётся единственным источником проверки диапазона и canonicality: отсутствие base важно, поэтому неканонический результат faults как #GP, не #SS. Не добавлять `67`, другие индексы, mod01/10, пары, V3 или owner/MMU integration.

Приёмка сравнивает direct V2/shared IR/interpreter для обоих направлений, шести data GPR, четырёх scale и signed disp32 границ, проверяя независимую формулу, полный CPU/RAM/callback/retirement и pre-state alias `data=RBP`. Дополнительно проверить #GP start/end без callback, AddressOverflow отдельно, backend faults без commit, store+STOP, byte-8 target identity, окна 1..7 без эффектов, unsupported соседей и bounded dispatcher publication/continuation без повтора. Address32 sibling now has a separate exact S3-20bg profile; it does not broaden this Qword admission. Отчёт: [S3-20bf report](../reports/STAGE_3_S3-20bf_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bg. Address32 indexed no-base SIB с RBP index

Добавить только singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base `101` (нет base), index RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer, точная длина 9. Это отдельный профиль от 20bf: EA равен `(low32(RBP)*scale + signed disp32) mod 2^32`, затем zero-extends в 64 бита; ни base, ни RIP не участвуют. Декодированный `MemoryOperand`/`access_address_48` остаётся единственным источником EA semantics. Не добавлять другие индексы, `67`-соседей, пары или V3.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, всех шести data-регистров, четырёх scale и MIN/-1/0/1/MAX disp32, включая pre-state `data=RBP` alias и полный 64-bit payload. Проверяются high-half truncation, independent low32 formula, qword callback через 4 GiB, byte-nine identity, arch/backend no-commit, store+STOP и budget/actual-RIP continuation без повтора; partial windows 1..8 и unsupported neighbors не дают effects. Отчёт: [S3-20bg report](../reports/STAGE_3_S3-20bg_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20bh. Address64 direct-base qword MOV через RSI/RDI

Допускаются только singleton `48 8B/89 /r` длины 3 с ModRM `mod=00`, base `rm=110` (RSI) или `rm=111` (RDI), data RAX..RBP; индекс отсутствует, displacement нулевой. RSI/RDI допустимы только как адресная база, не как data/index. Не добавлять SIB, displacement, `67`, дополнительные prefixes, pair или V3. EA, canonicality и qword range остаются decoder-owned.

Приёмка покрывает оба направления, обе базы и шесть data GPR direct V2/shared IR/interpreter parity с полным состоянием, RAM, callback адресом/count и retirement. Проверить, что полный старый RSI/RDI задаёт адрес независимо от RIP/RSP/RBP и сам не меняется; load меняет только destination, store записывает полный qword. #GP для noncanonical start/end не вызывает helper даже при RSP/RBP data; AddressOverflow отдельно; backend errors без commit; store+STOP committed once; entry STOP/stop-RIP/budget zero без доступа; continuation с budget=1 продолжает от actual RIP без повтора. Partial windows 1..2 и соседние SIB/displacement/addr32/data-RSI-RDI формы не дают effects. Отчёт: [S3-20bh report](../reports/STAGE_3_S3-20bh_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bi. Address64 direct-base qword MOV через RSI/RDI с displacement

Допускаются только singleton `48 8B/89 /r disp8|disp32`: ModRM `mod=01/10`, `rm=110` (RSI) или `rm=111` (RDI), data RAX..RBP, длины 4/7. RSI/RDI остаются только адресными базами. Нет SIB, `67`, дополнительных prefixes, новых data/index регистров, pair или V3. Signed displacement и EA вычисляет только decoder-owned `MemoryOperand::access_address_48`.

Приёмка: load/store × обе базы × шесть data GPR × signed края disp8 и disp32; независимое ожидание `old_base.wrapping_add_signed(displacement)`; CPU/RAM/callback/retirement parity direct V2/shared IR/interpreter. Проверить отсутствие влияния RIP/RSP/RBP на адрес, сохранность базы и остальных GPR, полный qword store, #GP start/end без callback, AddressOverflow отдельно, arch/backend atomicity, store+STOP once, entry guards и continuation от actual RIP без replay. Last byte disp32 должен менять target; partial windows 1..3 и 1..6, запрещённые SIB/prefix/addr32/data-RSI-RDI формы не имеют effects. Продолжить dispatcher через disp8 и disp32 singleton по budget=1 без повтора. Отчёт: [S3-20bi report](../reports/STAGE_3_S3-20bi_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bj. Address32 direct-base qword MOV через RSI/RDI

Допускаются только singleton `67 48 8B/89 /r [disp8|disp32]`: ModRM `mod=00/01/10`, `rm=110` (ESI) или `rm=111` (EDI), data RAX..RBP, длины 4/5/8. EA decoder-owned: `(low32(old RSI/RDI) + signed displacement) mod 2^32`, затем zero-extension; перенос данных и qword helper остаются 64-битными. Полные RSI/RDI сохраняются. SIB, дополнительные prefixes, новые data/index registers, пары и V3 не добавляются.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, обеих баз, всех шести data-регистров и границ displacement; проверяет полное состояние CPU/RAM, один callback `(EA, 8)`, retirement, неизменность полной базы и независимую low32-формулу. Верхние 32 бита базы не влияют на EA; qword callback у `0xffff_fffc` непрерывен через 4 GiB. Backend failure не коммитит состояние/RAM, STOP после успешного store оставляет запись закоммиченной, entry guards не вызывают helper, частичные окна и соседние SIB/data-RSI-RDI формы не имеют effects. Dispatcher продолжает со следующей singleton по actual RIP без повторного обращения. Отчёт: [S3-20bj report](../reports/STAGE_3_S3-20bj_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются за пределами среза.

#### S3-20bk. Address64 indexed SIB с RSI/RDI base

Принимать только singleton `48 8B/89 /r sib` длиной 4: ModRM `mod=00/rm=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8 и data RAX..RBP. Address/data size — Qword. Не принимать 67, displacement, no-index/no-base, RBP/RSI/RDI как index, пары или V3. EA берётся только из decoded `MemoryOperand::access_address_48` и равна `old_full_base.wrapping_add(old_full_index.wrapping_mul(scale))`; чтение source registers происходит до commit destination.

Проверки direct V2/shared IR/interpreter покрывают load/store для двух bases, четырёх indices, четырёх scale и шести data GPR; сравнивают полное состояние, RAM, точный callback `(EA, 8)` и retirement. Включены pre-state aliases data=index и сохранение base (base RSI/RDI не может совпасть с data RAX..RBP), неканонический start и конец qword range без callback, AddressOverflow, entry STOP/zero budget без helper, store+STOP commit и budget continuation через NOP и существующую пару без повторного чтения. Descriptor rejects neighboring encodings и partial windows. Отчёт: [S3-20bk report](../reports/STAGE_3_S3-20bk_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20bl. Address64 indexed SIB с RSI/RDI base и signed displacement

Поддержать singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длина 5/8. Effective address остаётся семантикой decoder `MemoryOperand::access_address_48`: old full base + old full index × scale + signed displacement с wrapping u64. Не добавлять address override, другие базы/индексы, no-index/no-base, prefixes, pairs или V3.

Descriptor и host direct V2/shared IR/interpreter проверяют обе операции, оба base, все индексы/scales/data GPR и signed границы disp8/disp32, включая pre-state data=index и точный target/callback. Проверяются также два disp32 artifacts с различием только в последнем displacement byte и high-index wrapping u64 адрес. Dispatcher покрывает disp8 и disp32 continuation по фактическому RIP через NOP и существующую пару без replay. Добавить #GP на неканонический start/end без callback, AddressOverflow отдельно, faults без commit, store+STOP commit once, STOP/zero budget guards и частичные окна/invalid neighbors без эффектов. Отчёт: [S3-20bl report](../reports/STAGE_3_S3-20bl_2026-10-28.md). Остальные production owner/MMU, Stage 2/ISA, Mini, physical x86 и performance requirements остаются открытыми.

#### S3-20bm. Address32 indexed SIB с RSI/RDI base

Поддержать только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. EA задаёт decoder-owned `MemoryOperand::access_address_48`: `(low32(old base) + low32(old index) * scale + signed displacement) mod 2^32`, затем zero-extension; перенос qword остаётся 64-битным, полные RSI/RDI сохраняются.

JIT descriptor и host direct V2/shared IR/interpreter должны проверять load/store, оба base, все data/index/scale combinations для mod00 и signed disp8/disp32 boundaries. Подтвердить data=index pre-state, независимую low32 oracle, точные callback и retirement, contiguous qword callback на границе 4 GiB, изменение target при изменении ninth source byte, architectural/backend load и store fault atomicity, store+STOP commit, pre-entry STOP/stop-RIP/zero-budget без helper, actual-RIP budget continuation последовательно через длины 5/6/9 без replay, partial windows и forbidden neighbors. Результаты/команды: [S3-20bm report](../reports/STAGE_3_S3-20bm_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bn. Address64 indexed SIB с RSI/RDI base и RBP index

Поддержать только singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index ровно RBP, scale 1/2/4/8, data RAX..RBP, длины 4/5/8. EA берётся из decoder-owned `MemoryOperand::access_address_48`: old full-width base + old full-width RBP × scale + signed displacement с wrapping u64. Не расширять общую whitelist индексов; no-index, RSP/RSI/RDI index, address override, другие базы, pair/V3 остаются вне профиля.

Проверить direct V2/shared IR/interpreter parity, data=RBP pre-state alias, все bases/scales/data/операции и mod00/disp8/disp32. Добавить независимую формулу адреса, wrap, фиксированный disp32 byte7 identity, точные callback/retirement, #GP на noncanonical start/end, отдельный AddressOverflow, fault no-commit, store+STOP, STOP/budget guards, continuation по фактическому RIP через формы 4/5/8 без повторных эффектов, partial windows и invalid neighbors. Результаты: [S3-20bn report](../reports/STAGE_3_S3-20bn_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bo. Address32 indexed SIB с RSI/RDI base и RBP index

Поддержать singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index ровно RBP, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. Effective address берётся только из decoder-owned `MemoryOperand::access_address_48`: `(low32(old base) + low32(old RBP) * scale + signed displacement) mod 2^32`, затем zero-extension; перенос qword остаётся 64-битным. RSI/RDI base сохраняются; индекс RBP читается из pre-state и меняется только если он является load destination после успешного доступа.

Проверки должны покрывать load/store × обе базы × все data registers/scales × mod00 и signed disp8/disp32 edges, direct V2/shared IR/interpreter parity, точные адрес/count/retirement и pre-state `data=RBP`. Добавить high-half truncation, contiguous qword callback на `0xffff_fffc`, architectural/backend no-commit, STOP после успешного store, pre-entry STOP/zero-budget, continuation по actual RIP между 5/6/9-byte forms без replay, ninth disp32 byte sensitivity, partial windows и invalid neighbors. Отчёт: [S3-20bo report](../reports/STAGE_3_S3-20bo_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bp. Address64 no-displacement SIB с RSI/RDI index

Поддержать только singleton `48 8B/89 /r sib`: ModRM `mod=00/rm=100`, SIB base RAX..RBX, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длина 4. EA вычисляется только decoder-owned `MemoryOperand::access_address_48`: старые полные значения base + index×scale с wrapping u64. `data=base` использует pre-state base для EA и store payload; load в base меняет его только после успешного read. RSI/RDI остаются address-only indices и не изменяются; общая whitelist индексов и data-регистров не расширяется.

Проверить load/store × четыре low bases × RSI/RDI × все scales × шесть data regs через direct V2/shared IR/interpreter parity, включая полный CPU/RAM, независимый EA oracle, callback address/count и retirement. Добавить точную #GP проверку canonical start/noncanonical end, отдельный AddressOverflow без callback, architectural/backend fault без commit, successful store+STOP, entry STOP/zero budget, actual-RIP continuation singleton→NOP→pair без повторных эффектов и partial/invalid neighbors. Отчёт: [S3-20bp report](../reports/STAGE_3_S3-20bp_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bq. Address64 indexed SIB с low base, RSI/RDI index и displacement

Добавить только singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RAX..RBX, index ровно RSI/RDI, scale 1/2/4/8, data RAX..RBP, длины 5/8. EA остаётся decoder-owned `MemoryOperand::access_address_48`: старый полный base + старый полный index×scale + signed displacement с wrapping u64. Не менять общие whitelist и не принимать address32, stack/no-base, другие indices, prefixes, pairs или V3.

Проверить обе операции × четыре base × RSI/RDI × четыре scale × шесть data GPR на signed disp8/disp32 границах: parity direct V2/shared IR/interpreter, полный CPU/RAM, независимая формула и callback/count/retirement. Подтвердить data=base из pre-state, full-width high-index bits и u64 wrap, RSI/RDI preservation, последнюю disp32 byte sensitivity, #GP start/end без callback, отдельный AddressOverflow, guest/backend no-commit, store+STOP, entry STOP/stop-RIP/zero budget, partial/unsupported без эффекта и actual-RIP budget continuation по disp8/disp32 без replay. Отчёт: [S3-20bq report](../reports/STAGE_3_S3-20bq_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20br. Address32 indexed SIB с low base и RSI/RDI index

Поддержать только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RAX..RBX, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. EA остаётся decoder-owned через `MemoryOperand::access_address_48`: `(low32(old base)+low32(old index)*scale+signed displacement) mod 2^32`, затем zero-extension; перенос и данные — полные qword. RSI/RDI используются только как индекс и сохраняются; data=base использует pre-state для адреса/store, а load commit происходит только после успешного чтения.

Приёмка: direct V2/shared IR/interpreter parity CPU/RAM/callback/retirement; все операции × low bases × RSI/RDI × scales × data GPR и формы mod00/disp8/disp32; high-half truncation и modulo-2^32 wrap, включая product/addition wrap; qword crossing 4 GiB одним contiguous callback; final byte 9 меняет decoded disp32 target; noncanonical high halves не порождают #GP после address-size truncation (Dword EA плюс qword length также не может переполнить u64); architectural/backend load/store no-commit; store+STOP и entry STOP/stop-RIP/zero-budget; actual-RIP continuation через 5/6/9-byte формы без replay; partial windows и invalid neighbors без эффектов. Отчёт: [S3-20br report](../reports/STAGE_3_S3-20br_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bs. Address64 indexed SIB с RSP base и RSI/RDI index

Поддержать только singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base ровно RSP, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 4/5/8. EA остаётся decoder-owned через `MemoryOperand::access_address_48`: старые полные RSP + старый полный RSI/RDI×scale + signed displacement с wrapping u64. `data=RSP` адресуется и сохраняется из pre-state; load меняет RSP только после успешного чтения. RSI/RDI используются только как индекс и сохраняются.

Приёмка: direct V2/shared IR/interpreter parity для обеих операций × 6 data GPR × RSI/RDI × 4 scale × mod00/disp8/disp32 signed edges; complete CPU/RAM, независимый EA oracle, callback address/count и retirement. Проверить pre-state `data=RSP`, full-width index bits, #SS при noncanonical start/end для RSP-base до callback, #GP для low-base control, отдельный AddressOverflow, architectural/backend no-commit load/store, store+STOP, entry STOP/stop-RIP/zero budget, byte8 disp32 target identity и actual-RIP continuation по 4/5/8-byte формам без replay. Partial windows и forbidden neighbors должны останавливаться без эффекта. Отчёт: [S3-20bs report](../reports/STAGE_3_S3-20bs_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bt. Address64 indexed SIB с RBP base и RSI/RDI index, с displacement

Поддержать только singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base ровно RBP, index RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 5/8. `mod=00` с SIB base field 5 остаётся no-base и этим профилем не принимается. EA вычисляется исключительно decoder-owned `MemoryOperand::access_address_48`: старый полный RBP + старый полный RSI/RDI×scale + signed displacement с wrapping u64. Значение data-register берётся из pre-state для store и адресной базы; load коммитит destination только после успешного чтения.

Приёмка: direct V2/shared IR/interpreter parity обеих операций × шести data GPR × RSI/RDI × четырёх scale × signed disp8/disp32 краёв; independent EA oracle, full CPU/RAM, callback address/count и retirement. Подтвердить #SS при noncanonical start/end для RBP-base до callback, #GP control на low-base, отдельный AddressOverflow, architectural/backend no-commit load/store, committed store+STOP, entry stop-RIP/STOP/zero-budget, partial windows 1..7 и actual-RIP continuation across 5/8-byte forms без replay. Добавить отрицательные соседи, включая no-base и лишний prefix, сохранив предыдущие профили. Отчёт: [S3-20bt report](../reports/STAGE_3_S3-20bt_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bu. Address32 indexed SIB с RSP base и RSI/RDI index

Поддержать только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base ровно RSP, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. EA остаётся исключительно decoder-owned `MemoryOperand::access_address_48`: `(low32(old RSP)+low32(old index)*scale+signed displacement) mod 2^32`, затем zero-extension; transfer и store payload остаются qword. Для data=RSP адрес и store source читаются из pre-state; load обновляет RSP только после successful read. RSI/RDI остаются address-only indices и сохраняются.

Приёмка: обе операции × data RAX..RBP × RSI/RDI × все scales × mod00/disp8/disp32, direct V2/shared IR/interpreter parity и полное CPU/RAM/callback/retirement сравнение. Подтвердить truncation high halves, modulo-2^32 product/add wrap, один contiguous qword callback на crossing 4 GiB, byte9 disp32 target identity, архитектурную/backend atomicity, store+STOP, entry stop-RIP/STOP/zero-budget, partial windows, invalid neighbors и actual-RIP continuation через 5/6/9-byte singleton без replay. Отчёт: [S3-20bu report](../reports/STAGE_3_S3-20bu_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

### S3-21. Подтвердить memory slice на физическом x86

Физический прогон отложен по решению владельца. Условия приёмки сохраняются.

1. Выполнить после локального завершения Stage 3 как финальный gate. Использовать завершённый S2-09 native memory profile. Проверить одинаковые входы, наблюдение всех заявленных GPR, нормализацию guest addresses и сравнение memory effects.
2. Добавить ручные и generated load/store cases, пересечения regions/pages, ошибки доступов и неполное наблюдение fault metadata.
3. Сравнить interpreter/JIT/native, повторить прогоны и сохранить полные результаты. Проверять helper и allocator clobbers вместе с памятью.

**Готово:** memory subset имеет независимое подтверждение. Если S2-09 не готов, задача остаётся заблокированной; synthetic snapshots, S0 oracle и локальное совпадение не заменяют её.

### S3-22. Учесть смену execution context

1. На изменениях режима, mapping identity/permissions, необходимых segment/control state и profile выполнить invalidate/relookup либо гарантированный exit.
2. Для отсутствующих production MMU/system features проверить contract через контролируемый fixture backend и обозначить интеграционную зависимость этапа 4.
3. Не считать неизменный RIP или одинаковые bytes доказательством пригодности старого compiled block.
4. Для reset и будущего restore/clone определить очистку или перепроверку cache. Архитектурные snapshots не содержат host code pointers; executable blocks создаются заново. Реализация полного VM snapshot относится к отдельному этапу.

**Готово:** context-sensitive cases не используют несовместимый перевод. Фактическое paging/TLB поведение не заявляется на основании fixture test.

#### S3-22a. Bounded fixture context generation

`ExecutionContextId` — caller-owned nonzero monotonic generation, а
`ExecutionContextSnapshot` combines it with the explicit JIT profile. Cached
single- and multi-entry APIs require this stamp and admit only the existing
`S0_09Fixture + Long64 + canonical48` profile; rejection occurs before cache
binding, compile, publish, or execute. The snapshot must remain frozen under an
owner lease for the whole synchronous dispatch. Mapping/backing IDs, versions,
and executable permission remain in the existing code dependency spans.

`TranslationKey` includes the context snapshot. A higher generation clears
resident entries and drops their published handles before a new lookup; a
same-generation profile change and generation regression are errors. Explicit
cache reset requires a higher generation. A reset/restore/clone therefore
causes fresh publication. GPRs, RFLAGS, budget, STOP-RIP and STOP pointers stay
out of the key. Tests cover same-context hit, changed-context miss at unchanged
RIP/source, old-handle drop, reset/republication, unsupported profiles with no
backend work, existing mapping/backing/permission invalidation, and state/control
changes without false misses.

This slice does not add a production MMU owner or live-owner candidate commit
for compilation that outlives/releases its snapshot. The caller's frozen
snapshot lease prevents owner mutation during the current synchronous
compile/publication path; revalidation is needed if a future async/unfrozen
path is added. Cached V2 memory load/store paths are not enabled. Physical Mini
checks remain deferred until final S3-21 after local Stage 3 work. Results are recorded in the
[S3-22a report](../reports/STAGE_3_S3-22a_2026-10-27.md).

### S3-23. Инвалидировать зависимости изменённого кода

#### S3-23a. Bounded fixture invalidation between dispatch calls

Реализован owner-to-cache notification path для bounded fixture cache между
завершёнными synchronous dispatch calls. `CodeInvalidation` покрывает
committed backing-content version change, mapping replacement/removal и
execute-permission change. `SingleEntryBlockCache` и `MultiEntryBlockCache`
снимают зависимые entries из lookup и сразу drop-ают опубликованные handles;
multi-entry counters освобождают resident code/metadata accounting. Поиск
backing ID учитывает aliases, а mapping events сопоставляют address-space и
mapping IDs.

Fixture owner сначала проверяет range/version increment, выполняет write,
продвигает backing version и передаёт notification до следующего snapshot.
Неуспешная запись не меняет bytes/version/cache. Cache borrow и API допускают
invalidation только между завершёнными calls, поэтому в этом срезе нет active
entry retirement protocol. Срез покрывает code owner fixture и register-only
S0_09 dispatch; unsafe V2 store для code writes не используется.

Локальные tests подтверждают changed bytes → old handle drop → новый compiled
code, alias-wide invalidation, failed-write preservation, remap на тот же guest
address, execute-permission revoke до повторного entry, unrelated backing hit и
reclamation resident handles/code/metadata. Owner mutation, racing publication,
interpreter/JIT/control writes, production MMU, DMA/device notifications и
active self-modifying code остаются вне среза и требуют последующей интеграции.
Результат: [S3-23a report](../reports/STAGE_3_S3-23a_2026-10-27.md).

#### S3-23b. Memory-pair fixture cache invalidation

`MemoryPairCache` now consumes the existing `CodeInvalidation` contract through
the same dependency matcher as the register-only single/multi-entry caches.
Backing-version changes follow backing identity across mapping aliases;
mapping replacement/removal and execute-permission changes use the existing
address-space/mapping semantics. A matching notification drops the one
inactive resident handle and returns the cache to empty. The caller supplies a
fresh dependency snapshot on the next `run`; `MemoryPairCache` does not own or
query the fixture owner.

Focused `od-jit` tests cover all notification variants, unrelated backing,
alias matching, handle drop, and republishing from an updated dependency
snapshot. The local macOS ARM64 `od-host` acceptance gate runs the published
two-site memory block, notifies invalidation after return, then republishes
successfully with a changed dependency version, fresh CPU state and fresh RAM.
The successful second publication proves the prior single-live MAP_JIT mapping
was released before republish.

This remains between-call fixture behavior. It adds no machine ownership,
production MMU/writer wiring, active SMC retirement, concurrency protocol, or
general memory/cache policy. S3-23, S3-24, and S3-25 remain open. See
[S3-23b report](../reports/STAGE_3_S3-23b_2026-11-03.md).

#### S3-23c. Generated V2 store в другой executable backing — fixture partial

Typed V2 store slot/handle размещается в общей bounded executable arena рядом
с cached V1 block slots, но остаётся отдельным от V1 entry и store API для
private data RAM. После stop-RIP → STOP → budget guards fixture owner проверяет и связывает полный qword
range, executable/write permissions, следующую nonwrapping backing version, исходные bytes/RIP с активным source lease и
пересечение source backing, включая alias. Ошибка preflight выдаётся только если generated helper дошёл до write; Store в активный
source отвергается до mutation. Успешный generated helper write меняет
фиксированный executable guest backing один раз; после возврата native call и
host result decoding owner продвигает version, отправляет
CodeInvalidation, и cache удаляет только старый target entry до следующего
lookup. V2 store slot вычитается из доступной cache capacity.

Локальный end-to-end fixture прогревает B и unrelated C, выполняет compiled
store A через alias B, подтверждает little-endian byte change, один write,
late-STOP commit, B invalidation/recompile с новым результатом и C cache hit.
Дополнительные cases проверяют active overlap, range/permission/version
preflight и backend failure без частичной записи/version/cache mutation.
Срез ограничен одним vCPU и фиксированным RAM; нет active SMC/retirement,
production MMU, MMIO, DMA, SMP или автоматического retry. Существующий unsafe
execute_memory_store_v2 private-data contract не ослаблен и для этого code
write path не используется. Результат: [S3-23c report](../reports/STAGE_3_S3-23c_2026-10-27.md).

#### S3-23d. Общая fixture transaction для interpreter/JIT/control writes — partial

Bounded `TrackedCodeFixture` направляет три писателя через общий путь:
`prepare_write` проверяет полный range, write permission, требуемую executable
permission, следующий backing version и overlap с active source;
`write_preflighted` выполняет одну all-or-nothing запись. Generated V2 store
передаёт заранее проверенный план; interpreter использует существующий
`od_isa::execute::step` и `GuestMemory::write`; control adapter вызывает тот же
`GuestMemory::write` между execution calls. После возврата interpreter/JIT
adapter фиксирует версию и применяет notification к cache до следующего
snapshot/lookup. Control adapter делает это до возврата caller.

Differential tests запускают один MOV [base],reg через interpreter и
production V2 host entry на независимых owners, сравнивают полный CPU state,
RAM, backing version, cache invalidation и последующее выполнение B. Отдельный
control-write case использует alias, сохраняет unrelated C и проверяет
backend failure без mutation/cache eviction. Active-source overlap в
interpreter также отвергается до commit. Это только test fixture: production
MMU/control owner, shared/asynchronous RAM, MMIO, DMA, SMP, REP/RMW,
compile-publication races и active-code retirement не реализуются. Stage 2
файлы не менялись. См. [S3-23d report](../reports/STAGE_3_S3-23d_2026-10-27.md).

#### S3-23e. Region-scoped executable-write notification and redispatch — partial

Generated V2 store entry принимает отдельную bounded code-region stamp. Owner
требует, чтобы регион заканчивался ровно на `next_rip` store, отдельно
запрещает target overlap с source bytes store artifact (включая guest aliases),
затем сверяет регион с backing. После committed store/result decoding применяет
`CodeInvalidated` ко всем зависимым cache entries, включая оба guest aliases,
и возвращает этот exit для redispatch до следующего lookup. Тест кеширует prefix и
tail в двух aliases, выполняет отдельный store entry, проверяет exact-once
commit и interpreter state/RAM parity, затем заново dispatch-ит tail и source
по новым bytes. Компилируемый artifact store содержит только сам store; tail
не исполняется native во время этой записи. Следовательно, это prerequisite
между calls для coherence, а не S3-24 retirement активного native tail.
См. [S3-23e report](../reports/STAGE_3_S3-23e_2026-10-27.md).

#### S3-23f. Cross-mapping qword write within one backing — partial

`TrackedCodeFixture` resolves a range covered by one mapping or by exactly two
guest-adjacent mappings. A crossing range is accepted only when both mappings
refer to the same backing ID and the second backing offset immediately follows
the first; preflight validates the complete backing range and write/execute
permissions on every mapping before it reserves one next backing version. The
commit remains one all-or-nothing slice copy and emits one backing notification.
The target does not require the write bytes or the decoder-backed code view to
stop at the page boundary; cache dependencies retain each mapping identity in a
separate span even though the code view reads a contiguous backing slice.

The native test warms B (`MOV RDX,imm64`) at page-end−6 and an alias, plus C on
another backing. A separate checked V2 qword store writes the immediate at
page-end−4 across two adjacent mappings. It verifies exact little-endian bytes,
one version increment/notification, eviction of B and its alias, a C cache hit,
and fresh native B/alias results from the new immediate. Negative cases cover a
missing, read-only, or non-executable second mapping, exhausted version, backend error,
different/noncontiguous backing, and arithmetic overflow; failures preserve CPU,
both backing halves, versions, and cache entries. Writes ending exactly at or
starting exactly on the boundary remain valid single-mapping cases. Resolver is
bounded to two mappings and fixed fixture RAM; no production MMU, more-than-two
mapping operation, MMIO/DMA, or concurrent writer support is implied. See
[S3-23f report](../reports/STAGE_3_S3-23f_2026-10-27.md).

1. Ввести единый путь уведомления committed memory writes и смены backing/mapping. Подключить interpreter, JIT и управляющие writes доступного среза.
2. Изменить generation/identity нужных code regions, удалить зависимые entries из lookup и сделать невозможным stale entry по старой ссылке.
3. Проверить writable executable region, aliases, пересечение нескольких страниц, запись из другого block и повторное отображение нового backing по тому же адресу.
4. Проверить изменение code version между capture/decode и publication; устаревший результат компиляции не публиковать.

**Готово:** повторное исполнение наблюдает новые bytes. Уведомление future DMA/device writers описано как обязательная integration point, а не фиктивно реализованное устройство.

Локальная регрессия проверяет отзыв права исполнения у уже опубликованного
tracked V2 store и у двухчастного исходного диапазона V3. При неизменных
байтах владелец фикстуры возвращает `Preflight(NotExecutable)`. Для V3
проверяется также отзыв права у хвоста при бюджете в одну инструкцию.
Отклонённые вызовы сохраняют состояние CPU, RAM и версии backing и не
выдают уведомлений. Выборка интерпретатора через фикстуру отклоняет
запрещённое отображение и сохраняет доступ через разрешённый алиас. После
восстановления разрешения тот же артефакт можно
исполнить. Оба точных теста обязательны в локальном селекторе. Их можно
запустить через `cargo test --offline -p od-host --lib
rejects_revoked_source_permission`. Общая S3-23 остаётся открытой.
Обе регрессии прошли в [локальном прогоне из 351 шага](../reports/STAGE_3_REPORT.md#local-tracked-source-and-reducer-run-2026-11-19-utc).

### S3-24. Обработать self-modifying code текущего блока

1. Создать case, в котором store изменяет ещё не исполненную guest instruction текущего compiled block, и case с изменением следующего блока.
2. После committed write, затрагивающего активные code dependencies, выйти с правильным guest state/RIP и продолжить через проверку актуального кода.
3. Проверить, что store выполнен ровно один раз, instruction count не удваивается, stale host tail не исполняется и переход через alias тоже учитывается.

**S3-24 остаётся открытым.** S3-24a добавляет один compiler-issued 23-byte native artifact с тремя decoder-validated instructions: `MOV RDX,imm64; MOV [RBX],RAX; MOV RCX,imm64`. Один typed V3 arena entry выполняет все три в одном generated call, но при успешной записи в уже исполненный prefix synchronously sets an invocation-local dirty marker, commits store RIP/budget/retirement once, and returns `CodeInvalidated` before stale native tail. The owner applies the reserved backing version/notification after result decoding and before the active handle is released. A later dispatch executes the tail from current bytes; re-entry observes the changed prefix. An unrelated destination completes the full three-instruction artifact. The store instruction itself may not overlap.

S3-24b permits an exact qword replacement of the immediate at artifact
offsets `15..23` in the pending `MOV RCX,imm64`. The 23-byte source
lease is checked before entry. The generated call commits the store
at RIP `entry+13` and returns `CodeInvalidated`. The owner applies the
backing generation and notification after return, so a fresh dispatch
uses the changed immediate. The current fixture also accepts a
destination split across two adjacent mappings that expose contiguous
offsets of that same backing. It rejects noncontiguous offsets, a
different backing in the split, changes to the tail opcode, partial or
shifted writes and overlap with the executing store.

Fault after the prefix preserves RIP at the store and the committed prefix; budget/STOP boundaries, STOP raised during a successful write, alias writes, and backend failure are covered. The future-tail case remains a fixture-only invalidation/redispatch demonstration, not a physical x86 SMC oracle. General multi-instruction compilation, production MMU/writers, and full S3-24 remain open. See [S3-24a report](../reports/STAGE_3_S3-24a_2026-10-27.md) and [S3-24b report](../reports/STAGE_3_S3-24b_2026-10-29.md).

#### S3-24c. Запись в отдельный блок по следующему RIP

ARM64 gate `active_code_v3_republishes_immediate_of_following_block`
прогревает блок по следующему RIP и его алиас. Нативный V3 store меняет
immediate в их общем backing через прямой адрес или алиас. Активный
трёхинструкционный блок завершается с оставшимся бюджетом 1. Уведомление
удаляет две записи, освобождает два слота и сохраняет записи двух
незатронутых блоков. Продолжение по прямому и алиасному адресу компилирует
обновлённые байты и совпадает с полным состоянием интерпретатора.
Гостевая запись выполняется один раз в каждом сценарии. Gate использует
имеющийся tracked owner fixture и локальный selector этапа 3.

В прогоне `stage3-20261119-053704-96e70bb3d6` прошли 359 из 359
локальных шагов, включая 341 точный Rust gate и 157 самопроверок.
Новый ARM64 gate прошёл 1/1. Ещё 46 шагов VMHost process остались
`not_run`, а физические x86 и Mac mini проверки отложены.
Отпечаток дерева с изменениями к `626e582` совпал до и после прогона
(`56fc39b807148a46d06197c2fa161702f269f6404c3c0079e44c53b7f2561c99`).
Summary SHA-256
`e40c8e39334120909d276dae4f10e518dadd501498f228f9dc17db5058663dba`
и input manifest SHA-256
`b5561279f6f34ae34de8390e99ba2c2ad19abe9ed888af67f3eb653b2a20ed81`
сохранены в `work/stage3-harness/` под ID прогона.

### S3-25. Реализовать eviction и безопасное освобождение

#### S3-25a. Synchronous published-handle lifetime partial

Для текущего одного vCPU `run_cached` удерживает borrowed cache entry,
compiled metadata и published handle от вызова backend до проверки его
decoded result/counters. `PublishedArenaBackend` держит borrow arena и slot
metadata на всём протяжении generated call и host result decoding. Безопасная
точка для eviction/clear/slot reuse — после возврата backend и обработки его
результата, до следующего lookup; cache API и borrow checker не допускают
сброса/замены текущего entry изнутри этого синхронного вызова.

Проверки capacity 1/2 выполняют A→B→A churn, включая соседний живой слот,
STOP до entry, STOP после backend возврата, injected backend/executor error,
clear и освобождение code/metadata accounting. Existing arena lease test
проверяет, что уничтожение backend не снимает MAP_JIT mapping пока live handles
его удерживают, а drop последнего handle позволяет открыть arena заново.
Ошибка выполнения удаляет entry только после возврата backend; STOP сохраняет
resident mapping. Результат: [S3-25a report](../reports/STAGE_3_S3-25a_2026-10-27.md).

Это не active invalidation/SMC, SMP reclamation, deferred retirement queue или
полный S3-25. Эти механизмы требуют отдельного протокола и остаются открытыми.

1. Развести недоступность для новых входов, active execution, references и физическое освобождение code/metadata.
2. Для одного vCPU использовать простой проверяемый retirement path на безопасных точках; не внедрять SMP reclamation framework.
3. Проверить capacity pressure, invalidation активного блока, выход по helper fault/STOP, удаление cache и повторное создание mapping.
4. После освобождения никакая entry/PC-map/link reference не должна вести в повторно используемую память прежнего блока.

**Готово:** стресс с малыми capacity не вызывает use-after-free, утечки retired blocks или исполнения освобождённого кода. Доказательства относятся к одному vCPU; SMP остаётся будущей работой.

#### S3-25c/S3-25d. Нормативный жизненный цикл и сводный стресс (один vCPU)

S3-25c записывает в [JIT_CONTRACT](../specs/JIT_CONTRACT.md) нормативный раздел
«S3-25c: block lifecycle for one vCPU»: четыре состояния (недоступен для новых
входов, активное исполнение, ссылки, физическое освобождение), переходы только на
safe point «после возврата и проверки результата, до следующего lookup»
(уничтожение последнего handle возможно и раньше, но тогда освобождение лишь
записывается; возврат слота происходит при следующем изменяющем доступе к арене),
сопоставление fixture (`MultiEntryBlockCache` + `PublishedArenaBackend`) и
production (`PagedFetchedLoadSession`, объявлен нормативным) с функцией и тестом
для каждого перехода, правило для будущих связей блоков (проверка поколения,
иначе задержка обнаружения не ограничена) и ограничения. S3-25d добавляет один
детерминированный стресс-тест на ёмкости 1 и 2
(`s3_25d_small_capacity_lifecycle_stress_matches_interpreter_and_frees_everything`).
S3-25 и S3-28 приняты с ограничениями (gate-ревью Claude Opus, high, 2026-11-18); ограничения — в строках S3-25/S3-28 таблицы статусов.
Отчёт: [S3-25c/d report](../reports/STAGE_3_S3-25cd_2026-11-18.md).

#### S3-25 — итог (принято с ограничениями, 2026-11-18)

| Пункт плана | Статус | Чем закрыт |
|---|---|---|
| 1. Разведение четырёх состояний | закрыт (один vCPU) | Нормативный раздел S3-25c в JIT_CONTRACT. Для каждого перехода в fixture и production назван функция и тест. |
| 2. Простой retirement на safe point | закрыт | Fixture: `pending_release`/`drain_pending_releases`, тесты `s4_15f_handle_dropped_under_a_borrow_is_deferred_not_leaked`, `cached_stop_and_executor_error_release_only_at_safe_return_boundaries`. Production: `retire_after_code_invalidation`, `s4_16i_the_retired_block_stays_owned_until_the_next_admission_reset_or_drop`. |
| 3а. Capacity pressure | закрыт | `s3_25d_*` (ёмкость 1 и 2), `arena_cache_a_b_a_churn_*`, `multi_entry_cache_reuses_sixteen_keys_and_evicts_exact_fifo_victim` |
| 3б. Инвалидация активного блока | частично — закрыт в текущем профиле | Точный V3 fixture: `active_code_v3_*`, s3_25d. Production, одна инструкция: `s4_16i_store_into_the_executing_code_page_*`. Общие многоинструкционные блоки — S3-24, открыт. |
| 3в. Helper fault и STOP | закрыт | s3_25d (ArchitecturalFault, committed STOP), `active_code_v3_guards_faults_aliases_and_committed_stop_are_precise` |
| 3г. Удаление cache и повторное создание mapping | закрыт | s3_25d (clear, drop backend при живых handles, reopen), `arena_lease_outlives_backend_until_last_handle_and_can_reopen`, `s4_15f_free_slots_return_to_the_initial_count_*` |
| 4. Ссылки entry и PC-map | закрыт | `direct_memory_v2_pc_map_survives_artifact_drop_and_arena_slot_reuse`, `active_code_v3_pc_map_survives_*`; в s3_25d отклонены все 84 stale-попытки. |
| 4. Ссылки link | выполнен «в текущем профиле» | Связей нет (S3-31 открыт). Правило проверки поколения на переходах — в S3-25c; обязательство переносится в S3-31 (У3). |

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

Локальная регрессия PC map закрепляет одинаковую проверку host PC для
`PcMap::lookup` и `snapshot_admissibility_at`. До исправления смещение
`1` давало `CanonicalBoundary`. Теперь невыровненные PC, конец кода и
смещения за ним возвращают `None`; допустимые PC сохраняют свою
классификацию. Проверка запускается через `cargo test --offline -p od-jit
--lib statepoints::tests`. Общая S3-27 остаётся открытой.

Локальная регрессия checked-memory dispatcher проверяет `GuestFault` на
границе с исполнителем. В путях с кешем и без кеша RIP должен указывать на
следующую незавершённую инструкцию артефакта, а остаток бюджета должен
позволять её начать. Неверный RIP, завершённый артефакт или исчерпанный
бюджет дают ошибку контракта хоста. Кеш освобождает запись после возврата
управления от исполнителя. До исправления оба теста принимали `GuestFault`
после завершения пары. `cargo test --offline -p od-jit --lib
guest_fault_requires_pending` проверяет допустимые отчёты и отклонения.

В локальный селектор Stage 3 входят три обязательных точных теста этих
границ. Самопроверка стенда требует каждую запись и отклоняет результат
`ignored` даже при успешном коде выхода Cargo. См.
[протокол тестирования](../testing/JIT.md#s3-27-local-fault-boundary-regressions).
Все три проверки и S3-29e прошли в [локальном прогоне из 348 шагов](../reports/STAGE_3_REPORT.md#local-fault-boundary-regression-run-2026-11-18).

#### S3-27b. Faulting next fetch after a committed two-site memory pair — partial

Native execution of either exact pair returns its canonical `A+6` CPU/RAM
snapshot. A separate interpreter step then fetches at this valid guest address
and receives a controlled architectural fault. The fixture compares against
two successful interpreter steps followed by the same faulting fetch, checks
full payload/state/RAM and confirms the faulting fetch is not retired or a
reason to repeat either pair memory effect. Unmapped/backend fetch failures
remain host errors. Exhausted budget and STOP gate continuation before fetch;
source `CodeView` exhaustion remains compile/decode metadata, not a guest fault.
This is fixture-boundary evidence only: the production owner-backed MMU and
multi-block dispatcher remain open.

См. [S3-27b report](../reports/STAGE_3_S3-27b_2026-10-27.md) and
[JIT testing protocol](../testing/JIT.md#bounded-s3-27b-next-fetch-after-memory-pair).

#### S3-27c. Host-PC attribution for the exact V3 active-code fixture — partial

`ActiveCodeBlockV3` builds a validated `PcMap` from checked ARM64 byte offsets.
It covers the complete emitted code with contiguous aligned ranges, records
commit boundaries at `A+10`, `A+13`, and `A+23`, and classifies commit,
post-helper, helper-failure, and shared exit ranges. The map is copied into the
published arena slot and survives compiler artifact drop and cache invalidation
while its generation is live; lookup through a stale handle fails after slot
reuse. This is diagnostic attribution only and adds no host-fault recovery.
Direct V2 maps, production owner/MMU integration, and full S3-27 remain open.

См. [S3-27c report](../reports/STAGE_3_S3-27c_2026-10-30.md) and
[JIT contract](../specs/JIT_CONTRACT.md#s3-27c-diagnostic-host-pc-attribution-for-the-exact-v3-fixture).

#### S3-27e. PC-map attribution for two-site V2 memory entries — partial

The compiler regression covers the exact store→load, load→store, and
load→load two-site sequences. It checks the complete emitted range partition,
instruction and shared-service attribution, guest RIPs, replay/snapshot
classification, and both retirement boundaries. Negative mutations must be
rejected for site attribution, shared service attribution, guest RIP, boundary
retirement count, and post-helper replay classification. A host regression
checks the published two-site map against the artifact, then republishes at a
distinct RIP/sequence and checks distinct, equal, and persistent map results
across artifact lifetime.

The focused `od-jit` and `od-host` tests passed 1/1 each on the local Mac;
independent Sol review passed after corrections to shared-service attribution
and the republication assertions. This is map-contract and publication-lifetime
evidence only. It does not test arbitrary host-fault recovery, guest fault
delivery, owner/MMU integration, Mini, performance, or complete S3-27. See the
[S3-27e report](../reports/STAGE_3_S3-27e_2026-11-01.md) and
[JIT testing reference](../testing/JIT.md#bounded-s3-27e-two-site-v2-pc-map-attribution).

#### S3-27h. Отказ для неподдержанной формы 0F C2

Декодер возвращает типизированный `Unsupported`, если после выбора
`0F C2` ни одна из поддерживаемых комбинаций префиксов не подошла.
Регрессия проверяет семь вариантов REX, точную длину отказа, fetch
только до опкода и приоритет ошибки чтения самого опкода.
[ISSUE-522 и границы проверки](../KNOWN_ISSUES.md#issue-522)

### S3-28. Реализовать STOP и event safepoints

1. Проверять cooperative STOP/events на документированных instruction/block/helper boundaries, сохраняя ограниченный work interval.
2. Для длинного блока гарантировать конечную задержку проверки; для REP/сложного fallback использовать только готовый restart/partial-progress contract этапа 2.
3. Согласовать total budget между blocks и interpreter. Проверить STOP до entry, внутри loop, рядом с helper/fault и при исчерпании budget.
4. Снять latency на representative busy path; не обещать заданные миллисекунды без измерения. Hardware timeout не считать точным гостевым STOP.

**Готово:** остановка возвращает корректное state и не теряет/повторяет side effects; ограничение задержки описано и проверено. Установка APIC/timers этим не заявляется.

#### S3-28 — итог (принято с ограничениями, 2026-11-18)

| Пункт плана | Статус | Чем закрыт |
|---|---|---|
| 1. Документированные границы и ограниченный интервал | закрыт | Матрица S3-28c: 6 случаев на 4 пути; неприменимые клетки объяснены. |
| 2. Конечная задержка в длинном блоке | закрыт | STOP через одну инструкцию, в т.ч. внутри пары: `s3_28c_stop_during_instruction_k_*`, `bounded_memory_dispatch_opt_in_bare_nop_bridge_*` (retired (1,0)). Event — через один блок, пару или bare NOP. REP/сложный fallback к JIT не применимы (только bare NOP), ограничение. |
| 3. Общий budget и STOP в ключевых местах | закрыт | `s3_28c_native_blocks_and_interpreter_steps_share_one_budget`, `forwards_remaining_budget_*`, `rejects_invalid_counters_*`; STOP до entry — клетки (a); в цикле — `s3_28c_stop_in_a_guest_loop_*`; рядом с helper/fault — `runtime_samples_stop_set_by_successful_helper_after_commit`, `helper_failure_after_prefix_*`; исчерпание — клетки (e). |
| 4. Latency на busy path | закрыт локально | S3-28d на M3 Max: release медиана 459 ns, p99 0.96–5.3 µs; миллисекунды не обещаются; только регистровые формы, один прогон на профиль; Mini — общий финальный gate этапа 3. |

## 8. Измеряемые улучшения

Для каждой задачи этого раздела сначала сохранить корректный baseline и сравнить минимальные варианты в одинаковых условиях. Записывать compile time, warm execution, code size, spills/helper calls и ресурсы, если они меняются. Выбор «по теории должно быть быстрее» не закрывает задачу.

### S3-29. Вычислять flags по потребности

1. На ограниченном integer subset представить pending flags с width, operation и достаточными исходными values. Не терять значения после перезаписи host registers.
2. Материализовать нужные bits при Jcc/ADC/SBB и все наблюдаемые определённые bits перед exit/helper/fallback/snapshot. Учитывать операции, которые изменяют только часть flags.
3. Проверить цепочки arithmetic → partial flag writer → consumer, spills, branch exits, fault и STOP. Сравнить с eager reference path и физическим x86.
4. Измерить eager и demand-driven варианты, включая стоимость snapshots/helpers. Выбрать default по данным; eager режим может сохраняться как диагностический эталон.

**Готово:** работающая demand-driven стратегия восстанавливает точное состояние; выбранная конфигурация имеет замеры. Если отложена сама реализация flags по потребности, соответствующее обязательное требование roadmap остаётся открытым, даже при успешном eager baseline.

Промежуточный результат S3-29a: добавлен отдельный uncached correctness candidate
для существующего bounded W64 register-only ADD/SUB/CMP, flag-preserving MOV и
JZ/JNZ. Native tests сравнивают его с eager и interpreter на проверяемых
цепочках, проверяют descriptor/lifetime, spills, guard/terminal materialization
и helper failure. Candidate не подключён к shared cache, не выбран default и не
имеет performance claim. Mini и измерения отложены до конца Stage 3 по
установленной методике; S3-29 остаётся незакрытой до оставшихся correctness
gates и итогового сравнения стратегий. Подробности и фактические проверки — в
[S3-29a report](../reports/STAGE_3_S3-29a_2026-10-29.md).

S3-29b connects this compiler as an explicit opt-in uncached multi-block
dispatcher over the existing bounded NOP-fallback loop. Native checks require
candidate PC-map evidence and compare full CPU state/accounting with eager and
interpreter execution across Jcc loops, both JZ outcomes, budgets, stop-RIP,
resume, event handoff and S3-28a priority. It does not change the eager default,
shared cache, ISA or memory profile. Mini/performance remain deferred to the end
of Stage 3; S3-29 is still open. See [S3-29b report](../reports/STAGE_3_S3-29b_2026-10-29.md).

S3-29c adds a separate exact-INC-RAX interpreter bridge for `48 FF C0` between
native register-only prefixes and native Jcc/suffix blocks. Candidate flags
must be canonical before the bridge; the interpreter uses a three-byte bounded
fetch and retires exactly one instruction. Host parity covers both CF values,
literal INC flags, both branch outcomes, budgets and stop/resume at every
reachable instruction boundary. Event requests are checked before INC and
remain pending on yield; STOP-RIP and atomic STOP retain priority over a
simultaneous event. A
synchronous interpreter step has no deterministic executor callback after
retirement, so event-after-step timing is not independently injected; STOP-RIP
at the post-INC boundary and normal next-entry event guards cover continuation.
The eager default, cache and Mini/performance status do not change. See the
[S3-29c report](../reports/STAGE_3_S3-29c_2026-10-29.md).

S3-29d adds fixture-scoped uncached handoff evidence for the existing W64
register arithmetic compiler, including two-writer ADD→SUB and ADD→CMP chains,
the exact V2 `48 8B 03` checked load, and native JZ/JNZ suffix. Candidate/eager/interpreter paths share the remaining budget and
continue at actual RIP; the real `PublishedMemoryBlockExecutor` performs the
load between released native mappings. The test covers ADD/SUB/CMP, both branch
outcomes, canonical flags at the helper handoff, #PF/#GP/backend-error behavior,
repair and retry, callback event/fault priority, and STOP/budget guards. It adds
no default, cache, owner/MMU, or general dispatcher route. Full S3-29 remains
open; Mini and performance checks stay deferred until the end of Stage 3. See
the [S3-29d report](../reports/STAGE_3_S3-29d_2026-10-29.md).

S3-29e promotes this bounded handoff to one public uncached `od-jit`
coordinator over the exact same admitted forms. Eager remains the default;
demand-flags is explicit. The coordinator validates the full immutable view,
shares one budget/control context, preserves original guest RIPs, and validates
executor counters and each phase boundary. The host differential test invokes
the coordinator for complete candidate/eager/interpreter runs and fault retry.
It does not add owner/MMU integration, cache admission, ISA forms or a
performance claim; Mini and performance remain deferred to the end of Stage 3.
See the [S3-29e report](../reports/STAGE_3_S3-29e_2026-10-29.md).

#### S3-29f: single-block eager/demand-flags measurement path

Added `od-bench flags-compare` for a fixed seven-case subset of accepted S0-09
register-only forms (`48 03 C0`, `48 03 C3`, `48 3B C3`, short JZ/JNZ):
replaced flag writers, an instruction-budget exit between writers, aliased ADD
operands, and JZ/JNZ taken and untaken outcomes. Both compilers must produce
the complete single guest block. The harness compiles and interpreter-checks
all seven cases, then validates the full eager/candidate profile against the
interpreter before it times any case. It compares full `LongModeState`, exit
continuation, remaining budget, and retired instructions with the interpreter.
It then measures
repeated execution of already published eager and uncached candidate blocks;
state reset, compilation, publication and outcome checks stay outside timed
calls. Pair order alternates, work counts match, each accepted series has 20
pairs with at least 200 ms per variant, and one MAP_JIT mapping is published
and dropped at a time. Compile/publication costs and emitted code sizes are
recorded separately. Three process runs completed. All seven cases had
cross-run median paired candidate/eager ratios above 1; the aliased ADD case
was near parity at 1.003352. Candidate emitted 40–47% more code bytes across this
profile. These are narrow single-block observations, not a production policy
or general performance conclusion. Timed calls include the safe wrapper,
runtime state materialization/snapshot, and arithmetic-flags helper work. This
does not include dispatcher/shared-cache lookup, interpreter fallback,
checked-memory helpers, or guest-memory workloads. S3-29 remains open. Mini and
physical-x86 checks remain deferred to the end of Stage 3. See the
[S3-29f report](../reports/STAGE_3_S3-29f_2026-10-30.md).

#### S3-29g: cached NativeRegisterV1 eager/candidate dispatch comparison

Add the fixed-profile `od-bench cached-flags-compare` measurement through the
existing eager and demand-flags-candidate cached NativeRegisterV1 entrypoints.
Before timing, compare every workload's full CPU state, exit, retirement and
remaining budget against the interpreter. Measure empty-cache first dispatch
separately from resident execution; call the latter resident only when prewarm
and per-sample counters prove hits with no new miss, compile, publication or
replacement. Use the same one-entry cache and publication policy, alternate
20 paired sample order with identical work, calibrate summed samples to at
least 200 ms, and complete three release-process runs on the same current
host. Drop each published mapping before the paired strategy is published.
Record code size, fallback/native retirement, raw timings, counters, profile
and source hashes, host/toolchain and timer boundaries. The fixed profile is
limited to NativeRegisterV1 flag-writer replacement, partial/preserving
writers, both Jcc outcomes, bounded back-edge dispatch and bare-NOP fallback.
Run the explicit release `--preflight-only` gate first; it checks exact
NativeRegisterV1 admission, compiler probes, and both strategies against the
interpreter before any timed samples.
This evidence does not select a global default or cover memory helpers,
physical x86, Mini, or the complete Stage 3 performance gate. See
[`BENCHMARKS.md`](../testing/BENCHMARKS.md#s3-29g-cached-nativeregisterv1-dispatcher-comparison)
and the [S3-29g report](../reports/STAGE_3_S3-29g_2026-10-31.md). Three M3
Max release runs completed after interpreter/cache preflight. Cold per-case
paired median ratios ranged 1.0354–1.1682; resident ratios ranged 0.9993–1.0090.
Candidate code was 28.2–57.5% larger. This is narrow profile evidence only;
S3-29 remains open for broader strategy selection and the Stage 3 performance
gate.

### S3-30. Выбрать register allocator

1. На измеренных горячих workloads определить цену spills/reloads и helper clobbers текущего простого allocator.
2. Сравнить минимальные варианты: текущий baseline и простое улучшение, например локальное удержание values или linear scan, если именно это оправдано профилем.
3. Проверить correctness при максимальном давлении на registers, разных paths, pending flags и helper calls. Сравнить compile time и code size вместе с warm throughput.
4. Если усложнение не даёт устойчивой пользы, сохранить простой вариант и записать результат эксперимента.

**Готово:** выбран простой allocator, удовлетворяющий измеренным требованиям; решение воспроизводимо. Наличие сложного allocator не является критерием успеха само по себе.

#### S3-30a. Расширить correctness coverage allocator liveness

Выполнен только correctness-coverage срез, без изменения allocator algorithm или runtime compiler behavior. Table-driven matrix в [`allocator.rs`](../../crates/od-jit/src/allocator.rs) проверяет определения и использования `Constant`, `ReadGpr`, `Integer`, `Compare`, `CheckedLoad` и `WriteGpr`; helper boundaries до/после definitions и uses; одновременную живость register homes и spills; повторное использование capture slots; бюджеты 0..=4. Сохранённый native helper-clobber probe прошёл как ограниченная transport/ABI smoke-проверка, без утверждения, что его sum-only assertion проверяет значения по отдельности.

Проверки: `cargo fmt --all -- --check`, `cargo check -p od-jit --all-targets --offline`, `cargo test -p od-jit --offline` (275/275), targeted `od-host` helper-clobber probe (1/1), `git diff --check`; Graphify обновлён, независимое Sol/high review чистое. ISSUE-039 закрывает только названный пробел def/use/helper-boundary coverage; pending-flags и прочие не проверенные paths остаются вне доказанного результата. Это не allocator comparison, performance evidence или выбор реализации; измерительная часть S3-30 остаётся открытой.

#### S3-30p3. Проверить индивидуальное сохранение allocator values через helper clobber

Test-only native probe теперь записывает и проверяет отдельно все шесть W64 значений после helper clobber: четыре register homes и две загруженные из spill slots values. Проверка выполнена после каждого из двух wrapper calls; observation buffer очищается перед вторым вызовом. Суммарный return contract `21`, nested helper path и GPR/SIMD/SP canaries сохранены. Изменений runtime allocator, compiler policy, production API или guest semantics нет.

Проверка: `cargo test -p od-host --lib --offline tests::allocated_register_and_spill_values_survive_helper_clobbers -- --exact` (1/1), `cargo fmt --all -- --check`, `git diff --check`; Graphify обновлён, независимое Sol/high review кода и test output чистое. Это закрывает только риск, что sum-only assertion не заметит перестановку или порчу отдельных values в данном bounded probe. Pending flags, другие pressure/path combinations, allocator policy/performance comparison и весь S3-30 остаются открытыми. См. [S3-30p3 report](../reports/STAGE_3_S3-30p3_2026-11-01.md) и [focused test output](../reports/data/stage3/s3-30p3/focused-test.log).

Дополнительный cost-probe срез сравнил `register_homes` budget4 и diagnostic budget0 на одном frozen register-only multi-helper block с двумя ZF initial states. Три release процесса сохранили 60 compile и 240 execution pairs; все стороны прошли 200 ms floor, корректность сверялась по полной CPU state и документированной end-of-block exit normalization. Compilation, publication/destruction и repeated execution измерены отдельно. Это bounded descriptive evidence: разброс пересекается, JZ+0 не разводит taken/fall-through targets, а allocator winner/default не выбран. Stage2 cargo/rustc процессы наблюдались в точечных снимках всех трёх runs, без per-pair timestamps; runtime working-tree status неизвестен. См. [S3-30a allocator cost report](../reports/STAGE_3_S3-30a_2026-11-01.md) и [archived evidence](../reports/data/stage3/s3-30a/). S3-30 остаётся открытым для broader workload evidence и принятия решения.

#### S3-30p4. Pending flags при JZ/JNZ с разными allocator budgets

Bounded test-only slice расширил existing `ADD RAX,RCX → MOV RAX,7 → JZ/JNZ` fixture на `register_homes` 0..4. Четыре inputs покрывают оба исхода обеих ветвей; Jcc должен материализовать flags от ADD, сохранив исходные helper operands после перезаписи RAX. Сравниваются candidate, eager и interpreter state/accounting на бюджетах 0..4; проверяются allocator locations четырёх temps (budget0 — четыре spills, intermediate budgets — homes и spills, budget4 — четыре homes), точные captured ADD operands/call count, entry-stop без helper и STOP/budget continuation без повторного retirement prefix. Для non-taken branch полное продолжение через dispatcher проходит admitted fallthrough MOV RBX,imm64.

Focused host test прошёл 1/1; `cargo fmt --all -- --check`, `git diff --check` и Graphify update завершились успешно. Это только локальное correctness evidence для названного W64 fixture и opt-in candidate; не доказывает сохранение произвольных live values, все allocator paths, physical x86, производительность или выбор allocator/default. Runtime/compiler/API и production policy не менялись. S3-30 остаётся открытым для broader pressure/path correctness и allocator cost/performance evidence. См. [S3-30p4 report](../reports/STAGE_3_S3-30p4_2026-11-01.md), [JIT test method](../testing/JIT.md#s3-30p4-jzjnz-pending-flags-under-allocator-pressure) и [saved focused output](../reports/data/stage3/s3-30p4/focused-test.log).

#### S3-30b. Bounded distinct-branch allocator-configuration measurement

Три release процесса на Apple M3 Max завершили frozen `distinct-jz-fallthrough` профиль; все 360 измеренных compile/execution sides прошли 200 ms floor. По 60 raw budget0/budget4 pairs в каждом наборе pooled ratio median/range составили 0.996036170827 (0.962808301520–1.045968410700) для compile, 1.019455351209 (0.981907318288–1.048155961189) для JZ-taken и 1.038610853426 (1.008862994910–1.086055631502) для JZ-fallthrough. Каждая native сторона исполняла 8,388,608 вызовов. Budget4 использовал 4 home values/10 spills и emitted 3,052 bytes; diagnostic budget0 — 0 homes/14 spills и 3,040 bytes; frame обоих вариантов — 288 bytes.

Это только bounded budget4-versus-budget0 configuration-cost evidence на одном Jcc-terminated block: admitted fallthrough `MOV RBX,imm64`, dispatcher и последующие блоки не входят в timing. Это не marginal spill cost, не сравнение allocator algorithms и не основание выбирать winner/default; runtime working-tree status каждого прогона остался неизвестным (`null` после Git metadata timeout). S3-30 остаётся открытым для более широких workload correctness/performance evidence. Детали протокола — в [BENCHMARKS S3-30b](../testing/BENCHMARKS.md#s3-30b-bounded-distinct-branch-allocator-configuration-profile); итоговые сырые пары, run/log/calibration fingerprints и pooled summaries — в [S3-30b report](../reports/STAGE_3_S3-30b_2026-11-01.md) и [suite manifest](../reports/data/stage3/s3-30b/suite-manifest.json).

### S3-31. Оценить прямые связи блоков

1. Сначала измерить долю dispatcher/lookup в representative multi-block workload. Переходы через dispatcher уже должны корректно работать.
2. Если direct linking обоснован, реализовать один ограниченный способ связи с проверенными lifetime и invalidation dependencies. Не patch-ить active code произвольно.
3. Проверить удаление target, invalidation source/target, context change, code-memory reuse и STOP/budget на linked path. Условие закрытия — правило S3-25c: проверка поколения на переходе между блоками или отвязка на safe point.
4. Сравнить latency, compile/link cost, code size и цену invalidation. При отсутствии пользы оставить dispatcher-only и документировать решение.

**Готово:** выбранный способ переходов проверен и измерен. Direct linking необязателен; корректные межблочные переходы обязательны.

#### S3-31a. Bounded warm dispatcher phase evidence

Для измерения доли dispatcher/lookup добавлен opt-in `warm-dispatch-phases`.
Он фиксирует S3-17 case-list и trace hashes, а `recorded_multiblock` и
`self_loop` держит отдельными strata. В каждой stratum cold ordinary prewarm
создаёт resident FIFO(16) entries, затем ordinary и observed preflight и
парные samples идут по одному общему прогретому engine/arena. Preflight
сверяет полное состояние и результат с interpreter oracle; preflight фиксирует
counter snapshots отдельно для каждого вызова, а измеренный side проверяет
агрегированный delta по числу visits и sweeps. Подробные frozen input/visit
identities, hashes, таймеры, фазы, residual и команда находятся в разделе
[BENCHMARKS S3-31](../testing/BENCHMARKS.md#s3-31-bounded-warm-dispatcher-phase-harness).

**Результат:** три bounded release runs по двум отдельным frozen strata
завершены; cross-run synthesis доступен в [отчёте S3-31](../reports/STAGE_3_S3-31_2026-11-01.md)
и [evidence manifest](../reports/data/stage3/s3-31/suite-manifest.json).
Широкая приёмка S3-31 остаётся открытой. Harness не изолирует цену
наблюдателя или total lookup и не доказывает пользу linking; никаких
production решений из этого среза не следует.

#### S3-31b. Phase attribution на frozen S3-36f recurrence — bounded diagnostic complete

Добавлен explicit-observer entry и opt-in `cached-native-loop-phases` для тех
же двух S3-36f kernels и шести strata. Release preflight подтвердил полное
состояние ordinary/observed/interpreter, retirement, cache accounting и
`C × I` вызовов каждой из трёх фаз; zero-work, entry STOP, event и
pre-admission code mismatch не зарегистрировали фаз. Сами phase totals
содержат только `BlockBuildAndAdmission` (вызов block builder),
`KeySnapshotAndLookup` (minimum size/capacity, key construction, первый
snapshot validation, find/counter) и `HostExecutionBoundary` (вызов backend
execution wrapper). Повторная snapshot validation и `matches_code_view`,
RUN guards, stop/event/budget, fallback, post-call counter/retirement checks
и observer begin/end остаются в residual.

Три release processes собрали 60 alternating pairs в каждой stratum; все
720 sides прошли 200 ms floor. Median observed/ordinary full-call ratios и
медианные phase shares observed call:

| Stratum | Observed/ordinary, median [range] | Build share | Key/snapshot/lookup share | Execution-boundary share | Residual share |
|---|---:|---:|---:|---:|---:|
| `a_1` | 1.1174 [1.0434, 1.2720] | 21.01% | 42.72% | 4.42% | 31.85% |
| `a_64` | 1.1182 [1.0914, 1.2488] | 21.73% | 44.09% | 4.53% | 29.71% |
| `a_4096` | 1.1163 [1.0885, 1.1635] | 21.75% | 44.09% | 4.54% | 29.67% |
| `b_1` | 1.0900 [1.0173, 1.1276] | 32.54% | 38.64% | 5.49% | 23.30% |
| `b_64` | 1.0967 [0.9970, 1.1858] | 33.22% | 39.35% | 5.62% | 21.76% |
| `b_4096` | 1.0983 [1.0728, 1.1437] | 33.20% | 39.41% | 5.60% | 21.76% |

Это diagnostic breakdown на синтетических register kernels. Phase shares
включают timer-observer perturbation и не являются точными production shares;
observed/ordinary delta — net full-path difference, не isolated observer cost.
В `b_64` ratio range пересекает parity. `HostExecutionBoundary` включает
wrapper/result decoding и не равен чистому generated-code execution;
`KeySnapshotAndLookup` не включает повторную snapshot/key validation.
Результат не обосновывает direct linking или изменение runtime/cache policy.
Широкая S3-31 и S3-36 остаются открытыми. Измеренная обработка ошибок
ограничена pre-admission mismatch; errors внутри instrumented phases не
проверялись. См. [S3-31b report](../reports/STAGE_3_S3-31b_2026-11-01.md),
[release preflight](../reports/data/stage3/s3-31b-20261101/preflight-release.json),
processes [1](../reports/data/stage3/s3-31b-20261101/process-1.json),
[2](../reports/data/stage3/s3-31b-20261101/process-2.json),
[3](../reports/data/stage3/s3-31b-20261101/process-3.json), and the
[suite manifest](../reports/data/stage3/s3-31b-20261101/suite-manifest.json).

#### S3-31c. Run-local prepared-key reuse — bounded opt-in profile

Commit `dcd1bfe5db813a2647c100c4c64a008dca9bd21c` adds the prepared-key reuse
candidate. A correctness-only release preflight checks six synthetic loop
strata against the ordinary dispatcher and interpreter, including exact guest
state, retirement, and remaining-budget parity; six partial/control cases
cover later-iteration boundaries, STOP before XCHG, STOP at JNZ, and wrapping
operand reuse. Each main-path candidate case prepares one key and reuses it
`iterations - 1` times. The untimed preflight was rerun at the same clean
committed source revision and fingerprints as the timing processes, using a
temporary detached worktree.

Three release processes recorded 20 paired baseline/candidate/interpreter
triples per stratum, 60 triples per stratum overall. All 1,080 measured sides
passed the 200 ms floor; minimum was 223,746,544 ns. The measured path includes
candidate memo setup/use/drop, but excludes state reset and CodeView/control
setup. Candidate/baseline and candidate/interpreter ratio medians [ranges]:

| Stratum | Candidate / baseline | Candidate / interpreter |
|---|---:|---:|
| `a_1` | 1.1998 [0.8821, 1.3680] | 4.7640 [3.5393, 5.0147] |
| `a_64` | 0.3532 [0.2257, 0.5166] | 1.4814 [1.0263, 2.2681] |
| `a_4096` | 0.3416 [0.3310, 0.3520] | 1.4330 [1.3094, 1.5237] |
| `b_1` | 1.1352 [1.0082, 1.1870] | 3.7066 [3.0948, 4.6654] |
| `b_64` | 0.2727 [0.2549, 0.2876] | 0.9184 [0.8491, 0.9749] |
| `b_4096` | 0.2591 [0.2428, 0.2760] | 0.8741 [0.8217, 0.9217] |

The candidate remains slower than the interpreter for all `a` strata and
`b_1`; only `b_64` and `b_4096` have candidate/interpreter ranges below 1.
These synthetic results do not establish broad workload speedup or justify a
runtime/default-policy change; the candidate remains opt-in and S3-31 remains
open. See [S3-31c report](../reports/STAGE_3_S3-31c_2026-11-01.md),
[preflight](../reports/data/stage3/s3-31c-20261101/preflight-release-final.json),
[process runs](../reports/data/stage3/s3-31c-20261101/), and the
[suite manifest](../reports/data/stage3/s3-31c-20261101/suite-manifest.json).

#### S3-31d. Проверка совпадающего снимка зависимостей

`TranslationKey` теперь сравнивает текущие зависимости с проверенной
упорядоченной копией в ключе. Точное совпадение проходит без выделения
памяти и сортировки. Остальные снимки проходят прежнюю проверку.
В трёх локальных release-раундах медианное нормированное время
прогретого вызова снизилось на 9,3–16,3% по шести профилям S3-36f.
Сырые времена пересекли равенство в четырёх профилях, а медианное время
JIT осталось выше времени интерпретатора. S3-31 и S3-36 остаются открытыми.
См. [замеры и критерии сравнения](../reports/STAGE_3_S3-31d_2026-11-19.md).

#### S3-31e. Повторный замер prepared-key reuse после S3-31d

Три локальных release-прогона на `b142e96` сравнили обычный cached JIT,
prepared-key JIT и интерпретатор. Штатный валидатор принял 60 троек на
профиль. На четырёх профилях с 64 и 4096 итерациями все замеры показали
выигрыш prepared-key пути у обоих вариантов сравнения. На одном проходе
его медиана на 8,2–12,8% выше обычного JIT. Режим остаётся opt-in.
Порог окупаемости и более широкие нагрузки остаются открытыми.
См. [отчёт и сырые данные](../reports/STAGE_3_S3-31e_2026-11-19.md).

#### S3-31f. Подготовленный ключ без копирования на первом проходе

Opt-in диспетчер переносит готовый `TranslationKey` в кеш текущего вызова
и хранит рассчитанную нижнюю границу размера ключа. Повторный проход
сохраняет проверку лимита и зависимостей. Все шесть профилей прошли
предварительную проверку. В измеренных `a_1` и `b_1` медианное время после
нормировки на обычный JIT снизилось на 10,8% и 6,8% соответственно.
Медианы обновлённого пути отличались от обычного JIT менее чем на 0,8%
в каждом раунде. Режим остаётся opt-in. См.
[отчёт и сырые замеры](../reports/STAGE_3_S3-31f_2026-11-19.md).

#### S3-31g. Повторное использование записей при чередовании двух RIP

Штатный S0-09 dispatcher в пределах одного RUN повторно использует
индекс текущего или предыдущего блока. Связь проходит через обычный
диспетчер с проверкой снимка, кода и условий выхода при каждом входе.
Локальные тесты проверяют точный порядок вызовов под FIFO и LRU с
вместимостью в одну и две записи. Тест LRU на трёх записях проверяет
индексы и порядок кеша после попаданий в среднюю запись.
В двух измеренных циклах с двумя RIP
нормализованное warm-время составило 9,05% и 7,36% прежнего времени, а
cold-время составило 20,36% и 7,52%. На одноблочных warm-циклах в 64 и
4096 итераций медианы выросли на 2,30% и 4,16%.
[Отчёт, ограничения и исходные данные](../reports/STAGE_3_S3-31g_2026-11-19.md)

#### S3-31h. Повторные замеры одноблочных циклов

В одном процессе текущий путь S0-09 занял на 1,9–2,6% больше времени
на длинных одноблочных циклах. Отложенный пересчёт индекса дал около
1% выигрыша в этом сравнении. На двух RIP в раздельных сборках у
прототипа выше нормализованное отношение JIT/interpreter. Сохраняется
текущая реализация. На актуальном снимке
JIT быстрее интерпретатора в cold и warm для одного и двух RIP
на 64 и 4096 итерациях. [Замеры и воспроизведение](../reports/STAGE_3_S3-31h_2026-11-19.md)

#### S3-31i. Прогретые циклы S0-09 на M1 Pro

На `5c92bf7` сравнили исходное и текущее тела диспетчера, добавив
повтор запуска того же подписанного файла как контроль A/A. В
шести раундах `same_64` и `same_4096` заняли на 2,01% и 2,82%
больше времени по парным медианам JIT. Их диапазоны лежат выше
разброса A/A. Два потока с двумя RIP заняли 8,23% и 6,70% времени
исходного пути. Текущая реализация сохранена. ISSUE-520 открыта.
[Замеры и воспроизведение](../reports/STAGE_3_S3-31i_2026-11-19.md)

#### S3-31j. Запись memo при переходе на тот же RIP

Кандидат пропустил запись пары RIP и индекс при переходе на тот же
RIP. На M1 Pro медианы JIT на `same_64` и `same_4096` составили
1.0001 и 1.0002 от исходного времени. Диапазоны включают единицу и
перекрываются с A/A. Сохранён исходный диспетчер. ISSUE-520 открыта.
[Замеры и воспроизведение](../reports/STAGE_3_S3-31j_2026-11-19.md)

#### S3-31k. Аргументы вызова при попадании по сохранённому индексу

Диспетчер передаёт кеш, индекс и адрес memo напрямую. На M1 Pro
подготовка вызова сократилась с десяти инструкций до четырёх.
Парные медианы прогретого JIT снизились на 1,99–2,49% на двух
длинных профилях с одним RIP и на 2,62–3,25% на двух профилях
с чередованием RIP. Все четыре диапазона лежат ниже A/A.
Изменение принято. Для ISSUE-520 остаётся прямое сравнение
с диспетчером до введения memo.
[Замеры, проверки и машинный код](../reports/STAGE_3_S3-31k_2026-11-19.md)

#### S3-31l. Прямое сравнение с диспетчером до memo

Обе сборки используют `a4dbc7d`. В исходном плече восстановлена
только функция `run_cached_store_controlled` из `ac3395da`. На
M1 Pro прогретый `same_64` укладывается в разброс A/A.
`same_4096` занял на 0,33% больше времени в прогретом режиме и
на 0,36% больше в первом запуске вместе с созданием сессии.
Два чередующихся профиля сохранили выигрыш в обоих режимах.
ISSUE-520 открыта для остатка на `same_4096`.
[Отчёт и архивы обоих прогонов](../reports/STAGE_3_S3-31l_2026-11-19.md)

### S3-32. Оценить локальные IR-преобразования

1. Выбрать только подтверждённые профилем возможности: local constant propagation, устранение избыточного state load/store или dead pure temporary.
2. Для каждой трансформации определить допустимые types/widths и запрет на изменение faults, guest instruction accounting, flags definedness и side-effect order.
3. Выполнить A/B correctness и timing с включённой/выключенной трансформацией, включая compile cost.
4. Не добавлять speculative loads, глобальный SSA или aggressive motion через guest fault points без отдельной необходимости и доказательства.

**Готово:** внедрены только полезные и корректные преобразования либо сохранён baseline с результатами сравнения. «Меньше IR operations» не заменяет измеренную пользу.

#### S3-32a. Одно чтение для точного aliased W64 ADD — bounded diagnostic A/B complete; no speedup claim

Commit `30febb1` добавил opt-in uncached correctness candidate только для
точной bare формы `48 03 C0` (`ADD RAX,RAX`, W64), закреплённой отдельной
`S3_32aAddRaxRaxAdmission` в
[`block.rs`](../../crates/od-jit/src/block.rs). Базовый
[`lower_instruction` arithmetic arm](../../crates/od-jit/src/lowering.rs)
эмитит два `ReadGpr` для decoded destination и source; candidate-specific
lowering читает общий aliased register один раз и использует этот temporary на
обоих входах `Integer`. `Integer` с arithmetic flags, вычисление результата,
`WriteGpr`, guest commit, guards и retirement сохранены. Baseline compiler и
обычный `lower` путь не изменены.

Candidate остаётся opt-in и uncached; baseline и candidate используют
`register_homes=4`. Diagnostic budget0 из S3-30b не является candidate для
этого сравнения. Default compiler, eager-flags default, cache admission/key и
production policy не менялись. Read reuse не распространяется через соседние
guest instructions, helper, memory effect или commit; allocation/PC-map
metadata вручную не редактировались. Другие alias patterns не входят в этот
срез; не заявляется сохранность не наблюдавшихся live values.

**Correctness result:** structural и normal verified compile-pipeline tests
подтвердили exact admission, один read для формы и отсутствие расширения на
negative/non-alias cases; od-jit прошёл 2/2. Host parity test сравнил
interpreter, baseline и candidate по полному state/flags, RIP/exit, remaining
budget и retirement для arithmetic edge values, STOP, budget exits и
continuation без повторного commit; od-host прошёл 1/1. Независимое code review
завершилось PASS. После benchmark-harness commit `cefd59c` команды
`cargo fmt --all -- --check` и
`cargo check -p od-jit -p od-host -p od-bench --all-targets --offline`
и `git diff --check` завершились с exit 0; Graphify update тоже завершился с
exit 0. Точные receipts приведены в отчёте.
Подробности — в
[S3-32a report](../reports/STAGE_3_S3-32a_2026-11-01.md).

Commit `cefd59c` добавил и прошёл review изолированный benchmark harness
`allocator-cost-s3-32a`. Три последовательных release process runs завершили
preflight, shared calibration и paired baseline/candidate A/B на frozen
S3-30b `distinct-jz-fallthrough` fixture (27 bytes; SHA-256
`72210fb82b0ffa9f88f10f63697a1051347d07587e3569028947119726fac8c7`). Каждый
run содержал 20 compile pairs и 20 native pairs на каждую branch stratum;
pooled данные: 60 pairs на set. Общие calls/side: 32,768 compile, 8,388,608
native; 64 warmup calls/publication. Все 360 timed sides прошли 200 ms floor;
minimum — 283,092,167 ns. Fixture preflights совпали, calibration bounds
пройдены; независимый raw audit проверил 3,074 assertions.

Candidate/baseline ratios рассчитаны из raw paired totals. Все диапазоны
пересекают 1, поэтому результат фиксирует только более низкие pooled medians
candidate на этом fixture, без подтверждённого speedup или default change:

| Set, n=60 | Candidate/baseline median (range) |
|---|---:|
| Compile | 0.9832762133712801 (0.9488835862269013–1.062649364816329) |
| JZ taken | 0.9878736671103938 (0.9749361861693031–1.0766848438450791) |
| JZ fallthrough | 0.9975100869584979 (0.9322534409534861–1.0375123643358157) |

Обе стороны использовали `register_homes=4`. Baseline/candidate: code 3,052 /
3,032 bytes, frame 288 / 272 bytes, `ReadGpr` 10 / 9, temps 14 / 13,
spills 10 / 9; helper capture/save/reload остались 3 / 3 / 3. Compile timer
включал candidate transform и artifact drop; publication/drop измерялись
отдельно; native timer исключал создание и validation state vectors. Все raw
reports, command/exit bindings, preflight/calibration/build and executable
receipt SHA-256s закреплены в
[S3-32a suite manifest](../reports/data/stage3/s3-32a/suite-manifest.json).
Сборка использовала source revision `958be7edf97dd0e421454630285dc4d6e711b2a7`
и SHA `97d0130cb448f4834840fcbbeadb449067c6b05a0d664c4d99d03e7f1a4898c3`;
build source tree был чистым. Runtime `working_tree_dirty=null` из-за git
status timeout в 2s; чистота runtime не подтверждена. Подробности и границы —
в [S3-32a report](../reports/STAGE_3_S3-32a_2026-11-01.md) и
[S3-30b report](../reports/STAGE_3_S3-30b_2026-11-01.md).

Эти данные относятся только к названному fixture, не к general workload,
allocator winner или production default. Mini checks отложены до конца Stage 3.
Stage 2 suite в этом срезе не запускался; параллельная активность Stage 2 не
является blocker. S3-32 и Stage 3 остаются открытыми.

**Холодный повтор на M1 Pro.** В одной подписанной release-сборке и при
`nice −5` объединённый путь от компиляции до первого нативного возврата
для candidate занял на 6,2–6,3% больше времени по парной медиане. Отдельные
контроли компиляции и прогретого исполнения дали снижение на 1,4–2,1%.
Для этого fixture сохраняем baseline. Принятие преобразований на более
широких нагрузках в S3-32 остаётся открытым.
[Методика, результаты и архив](../reports/STAGE_3_S3-32a_2026-11-01.md#first-native-return-on-m1-pro-2026-11-19)

### S3-33. Расширять lowering по готовности interpreter

1. По таблице S3-00 выбирать следующую готовую integer/branch/memory форму. Отдельно подтверждать widths, flags, faults и helper effects.
2. Для сложных поддержанных форм использовать собственный helper/interpreter fallback с точной materialization, если native lowering ещё не обоснован.
3. REP, atomics, system instructions, x87/SIMD и иные расширения не переводить приблизительно. Перед включением дождаться соответствующей semantics и oracle evidence этапа 2.
4. Для каждого profile публиковать counts native/helper/fallback/unsupported и не менять CPUID из-за одного наличия lowering.

**Готово:** выбранный JIT subset шире только за счёт реально проверенных форм; весь interpreter ISA не является обязательным native lowering этапа 3.

#### S3-33a. Exact native qword INC RAX

Добавлен отдельный uncached opt-in путь lowering только точного bare `48 FF C0`
через существующий `S0_09NopIncRaxAdmission`. Он представляет INC как W64 Add
с `FlagEffect::WriteMask`, сохраняет CF и обновляет PF/AF/ZF/SF/OF. Eager и
demand-flags candidate материализуют перед INC ожидающий полный writer; candidate
не вводит новый pending descriptor или helper ABI. Существующий S3-29c
interpreter bridge остаётся baseline. Native host parity, branch-consumer,
retirement, budget, STOP, PC-map и negative-form checks документируются в
[S3-33a report](../reports/STAGE_3_S3-33a_2026-10-30.md). Это один узкий
correctness slice; S3-33 и Stage 3 остаются открытыми, performance/Mini
проверки отложены до конца этапа.

#### S3-33b. Exact native qword DEC RAX

Добавлен отдельный uncached opt-in путь для точного bare `48 FF C8` через
`S0_09NopDecRaxAdmission` и отдельно названные eager/demand-candidate compile
и dispatch API. Admission не меняет INC-only `S0_09NopIncRaxAdmission`.
Lowering представляет DEC как W64 Subtract на единицу с частичной маской
PF/AF/ZF/SF/OF; CF и остальные bits сохраняются. Demand candidate материализует
предшествующий полный flags writer до DEC и сохраняет live allocator homes;
новых pending descriptor/helper ABI нет. Отдельные compiler и host differential
проверки покрывают полное состояние, retirement, оба CF, arithmetic/AF/PF
edges, JZ/JNZ, candidate budgets и STOP на entry/перед DEC после pending CMP;
также проверены точные отрицательные формы.
Срез частичный: он не выбирает default, не входит в shared cache и не содержит
performance claim. Mini и замеры отложены до конца Stage 3. См.
[S3-33b report](../reports/STAGE_3_S3-33b_2026-10-30.md).

#### S3-33c. Exact native qword NEG RAX

Добавлен отдельный uncached opt-in путь только для bare `48 F7 D8` (`NEG RAX`)
через собственный `S0_09NopNegRaxAdmission` и eager/demand-flags compile и
dispatch API. Admission для INC/DEC не расширяется. Lowering читает RAX,
создаёт W64 ноль и вычисляет `Subtract(0, RAX)` с полным `FlagEffect::Arithmetic`,
записывает RAX и коммитит инструкцию. NEG не потребляет старый CF; candidate
заменяет ожидающие полные flags новым полным результатом без нового descriptor
или helper ABI. Проверены boundary, branch, retirement и statepoint случаи;
Mini/performance отложены до конца Stage 3. См.
[S3-33c report](../reports/STAGE_3_S3-33c_2026-10-30.md).

#### S3-33d. Exact native qword ADD RAX, sign-extended imm8

Добавлен отдельный uncached opt-in profile для bare `48 83 C0 ib` с каждым
значением `ib` от `00` до `FF`. `S0_09NopAddRaxImm8Admission` проверяет
sign-extension каждого source byte против decoded immediate и не расширяет
generic, INC, DEC или NEG admission. Lowering использует decoder-owned значение как W64 Const и выполняет
Add с полным `FlagEffect::Arithmetic`; eager и demand-flags API используют
существующий IR/emitter и pending-flags path. Проверены все 256 байтов,
`ADD ...,0`, отрицательные imm8, полное состояние, pending CMP/ADD/JZ/JNZ с
различимыми outcomes, pending PC-map witness, successor full writer, budget и
STOP. Реальные результаты и оставшееся независимое ревью отмечены в
[S3-33d report](../reports/STAGE_3_S3-33d_2026-10-30.md); independent review
завершено без оставшихся замечаний. Mini и performance
проверки отложены до конца Stage 3; S3-33 и этап остаются открытыми.

#### S3-33e. Exact native qword CMP RAX, sign-extended imm8

Добавлен отдельный uncached opt-in profile только для bare `48 83 F8 ib`
(`CMP RAX, sign-extended imm8`, opcode `83 /7`) со всеми 256 байтами
непосредственного операнда. Admission сверяет raw byte с decoder-owned
sign-extended immediate; lowering использует W64 `Compare` с
`result=None` и полным `FlagEffect::Arithmetic`, без `WriteGpr`. Eager и
demand-flags APIs используют текущие IR/emitter/pending-flags механизмы;
generic, INC, DEC, NEG и ADD-immediate admissions остаются отдельными.
Проверены равенство/неравенство, неизменность всех GPR, branch consumers,
candidate pending-statepoint, следующий full writer, бюджеты и STOP. См.
[S3-33e report](../reports/STAGE_3_S3-33e_2026-10-30.md). Mini и performance
проверки отложены до конца Stage 3; S3-33 и этап остаются открытыми.

#### S3-33f. Exact native qword SUB RAX, sign-extended imm8

Добавлен отдельный uncached opt-in profile только для bare `48 83 E8 ib`
(`SUB RAX, sign-extended imm8`, opcode `83 /5`) со всеми 256 значениями
непосредственного операнда. Admission сверяет исходный byte с decoder-owned
sign-extended immediate; lowering использует это значение как W64 Const,
вычисляет `Subtract` с полным `FlagEffect::Arithmetic`, записывает RAX и
коммитит один раз. Eager и demand-candidate compile/dispatch APIs используют
существующий IR/emitter и pending-flags path; generic, INC, DEC, NEG, ADD и CMP
admissions остаются отдельными.

Проверены нулевой SUB и обновление flags, граничные signed/unsigned значения,
оба старых CF, AF/PF, сохранение DF/non-arithmetic flags, полное состояние,
RIP и retirement. Pending CMP→SUB и ADD→SUB имеют различимые JZ/JNZ outcomes;
также проверены candidate PC-map/statepoint, terminal SUB, successor writer,
budgets и STOP до SUB, после SUB и перед writer. Exact local harness расширен
тремя fail-closed Cargo gates при сохранении предыдущих девяти. Точные команды,
результаты и ограничения зафиксированы в
[S3-33f report](../reports/STAGE_3_S3-33f_2026-10-30.md). Mini и performance
проверки отложены до конца Stage 3; S3-33 и этап остаются открытыми.

#### S3-33g. Exact native qword NOT RAX

Добавлен отдельный uncached opt-in profile только для bare `48 F7 D0`
(`NOT RAX`). Admission сверяет всю форму и границы инструкции; NEG, другой
регистр, R8, память, ширина, дополнительные префиксы, truncation и branch
внутрь инструкции не допускаются. Lowering выражает NOT как `u64::MAX - RAX`
с `FlagEffect::Preserve`, затем записывает RAX и коммитит один раз. Runtime
emitter принимает только W64 `Subtract + Preserve + result`; это не расширяет
поддержку произвольных flag-preserving операций. NOT не вызывает flags helper,
не изменяет RFLAGS/definedness и не меняет существующий pending-flags descriptor
или его сохранённые операнды. Eager и candidate compile/dispatch APIs остаются
отдельными opt-in путями.

Покрыты точная admission, значения 0/MAX/signed extremes/alternating bits,
полное состояние CPU и флаги, повторный NOT, pending ADD/CMP → NOT → JZ/JNZ,
двойной NOT до branch, новый writer после NOT, STOP/budget/end-of-code/branch и
отсутствие helper-call/AfterHelper map для NOT. Mini и performance проверки
отложены до конца Stage 3. Результат, точные проверки, независимое ревью и
локальное harness evidence фиксируются в
[S3-33g report](../reports/STAGE_3_S3-33g_2026-10-30.md); S3-33 и Stage 3
остаются открытыми.

#### S3-33h. Exact native qword LEA RAX,[RBX]

Добавлен отдельный uncached opt-in profile только для bare `48 8D 03`
(`LEA RAX,[RBX]`). Admission одновременно сверяет точные bytes и decoded
shape: destination RAX W64, address size 64, base RBX, без index/displacement
и RIP-relative addressing; альтернативные эквивалентные кодировки, другие
регистры/widths/prefixes, truncation и branch внутрь инструкции отвергаются.
Lowering — только `ReadGpr(RBX,W64)` → `WriteGpr(RAX,W64)` → один commit.
Путь не обращается к данным памяти и не создаёт arithmetic/flags effects;
полные flags, definedness и pending-writer descriptor/operands сохраняются.
Eager и demand-flags candidate остаются отдельными opt-in API.

Host parity включает unmapped/noncanonical адресные значения, полный state,
данные памяти через rejecting probe, pending ADD/CMP → LEA → JZ/JNZ с
различимыми ветвями, LEA → NOP interpreter fallback → branch, последовательные
LEA, successor writer, PC-map, budget/STOP и terminal случаи. Три exact local
harness gates закрепляют admission, compiler и host test identities. Mini и
performance отложены до конца Stage 3; S3-33 и этап остаются открытыми. См.
[S3-33h report](../reports/STAGE_3_S3-33h_2026-10-30.md).

#### S3-33i. Exact native qword XCHG RAX,RBX

Добавлен отдельный uncached opt-in profile ровно для bare `48 87 D8`.
Admission связывает точные source bytes с decoded W64 RAX/RBX и отклоняет
эквивалентные кодировки, остальные регистры/ширины, self-exchange, memory,
префиксы, truncation и branch внутрь инструкции. Lowering сначала читает оба
старых значения, затем выполняет две перестановочные записи и один commit.
Emitter допускает dual-write только для этого полного упорядоченного IR-shape;
между записями нет canonical snapshot, retirement остаётся одной инструкцией.
Flags и pending-writer state сохраняются без helper; никаких утверждений о
memory atomicity нет. Eager и candidate compile/dispatch раздельны и uncached.
Host parity покрывает полное состояние, крайние/alternating значения, двойной
обмен, CMP → XCHG → JZ → последующий SUB на swapped values и STOP/budget. Native и
interpreter evidence относится только к локальной точной кодировке. S2-10
подтверждает XCHG semantics, но его reviewed physical fixture не подтверждает
именно `48 87 D8`. Четыре точных local gates закрепляют admission, emitter,
compiler и host parity. Mini и performance остаются отложены до конца этапа;
S3-33 и Stage 3 открыты. См.
[S3-33i report](../reports/STAGE_3_S3-33i_2026-10-30.md).

#### S3-33j. Exact native LEA RAX,[RBX+disp8]

Добавить отдельный opt-in uncached profile только для `48 8D 43 db`, все 256
значений displacement byte. Admission проверяет decoder-owned W64 RAX,
address64, base RBX, отсутствие index, scale 1, и точное равенство
displacement sign-extended i8; `48 8D 03` остаётся отдельным профилем.
Lowering: `ReadGpr(RBX)` → signed displacement constant → W64 `Add` с
`Preserve` и результатом → `WriteGpr(RAX)` → один commit. Это wrapping
register arithmetic: без memory operation, data access, canonicality/fault
checks или helper; flags, definedness и pending-writer descriptor/operands
не меняются. Eager/candidate compile/dispatch остаются раздельными.

Admission должна исчерпывать все 256 байтов и отклонять другие destination/base,
SIB, disp32, RIP-relative, address/operand width, prefixes, truncation,
старую no-displacement форму и branch внутрь кодировки. Host differential
сверяет полное состояние и retirement на displacement edges, wrapping и
noncanonical/unmapped значения, с нулём data callbacks. Проверить pending
ADD/CMP → LEA → JZ/JNZ, несколько LEA, последующий writer, terminal/fallback,
budget/STOP до и после, а также PC map. Emitter допускает только W64 Add или
Subtract с `Preserve` и результатом; прежние NOT/sub regression gates
сохраняются. Четыре exact harness gates добавляют admission, preserving
emitter, compiler и host identity; сохранены прежние identities, кроме
намеренного переименования старого NOT emitter gate. Местная дифференциальная
проверка подтверждает только эту exact encoding; физические байты не
подтверждены. Mini/performance отложены до конца Stage 3. См.
[S3-33j report](../reports/STAGE_3_S3-33j_2026-10-30.md).

#### S3-33k. Exact native MOV RAX,sign-extended imm32

Добавить отдельный opt-in uncached profile только для bare `48 C7 C0 id`,
точно семь байтов. Admission проверяет decoder-owned `MovImmediate`, W64 RAX,
`/0`, длину 7 и соответствие signed-extended decoded value исходным четырём
little-endian bytes. Использовать сохранённый decoded value; не повторять
расширение знака в JIT. Формы других `/digit`, destination/register/memory,
width, prefixes, `C6`, truncation и branch внутрь imm32 не входят. Selector
нового профиля отклоняет `48 B8 imm64`; прежний baseline B8 profile остаётся
неизменным.

Lowering — Constant→WriteGpr(RAX) и один commit; остальные GPR, RFLAGS и
pending flags сохраняются, без arithmetic, data-memory access или helper.
Host differential проверяет ADD/CMP→MOV→различимый JZ/JNZ.
Eager/candidate compile и dispatch APIs отдельные. Проверить репрезентативные
immediate/pattern values, full-state interpreter parity, сохранение флагов и
pending ADD/CMP перед MOV с последующим JZ/JNZ, несколько MOV и последующих
writers, exits/fallback, budget/STOP, truncation 1..6, PC map и отсутствие
arithmetic/memory/helper операций; сохранить B8 baseline regression. Не
заявлять exhaustive перебор 2^32 значений. Добавить admission/compiler/host
exact gates и fail-closed omission checks. Точного физического oracle для
`48 C7 C0` сейчас нет; зафиксировать только локальные differential evidence.
Mini/performance отложены до конца Stage 3; Stage 2 и owner/MMU вне scope.
См. [S3-33k report](../reports/STAGE_3_S3-33k_2026-10-30.md).

#### S3-33l. Exact native LEA RAX,[RBX+RCX]

Отдельный uncached профиль допускает только байты `48 8D 04 0B`: W64/address64, RAX destination, RBX base, RCX index, scale 1, displacement 0, длина 4. Предыдущие LEA профили остаются отдельными. Lowering читает RBX и RCX до записи результата, выполняет W64 Add с Preserve flags, пишет RAX и коммитит одну инструкцию. Data memory и helper не используются; guest address values не проходят canonicality/data-access path. Admission/decoder negatives и eager/candidate compilation покрыты локальными тестами; host parity проверяет representative operands, flags, pending CMP→LEA→Jcc, budget exhaustion, STOP at entry и нулевые data callbacks.

Доказательство ограничено локальными differential checks. Для точных байтов `48 8D 04 0B` нет отдельного physical fixture evidence; этот срез не расширяет Stage2, owner/MMU или Mini. Mini и performance остаются not_run до конца Stage 3. См. [S3-33l report](../reports/STAGE_3_S3-33l_2026-10-30.md).

#### S3-33m. Exact native LEA RAX,[RIP+disp32]

Отдельный uncached профиль принимает только `48 8D 05 dd dd dd dd` (7 байт):
W64/address64, RAX, `NextRip`, без index, scale 1 и signed displacement,
связанный с exact bytes и decode. Lowering вычисляет
`instruction.next_rip.wrapping_add_signed(displacement)`, записывает W64
constant в RAX и коммитит одну инструкцию. Flags, остальные GPR и pending
flags сохраняются; data access, canonicality check и helper отсутствуют.
Admission/compiler/host gates проверяют exact negatives, signed edges,
переменную длину предшествующей инструкции, разные guest RIP, wrapping,
полное interpreter/native состояние, pending CMP→LEA→JNZ и нулевые data
callbacks. Результат — только local differential evidence; отдельное physical
подтверждение exact bytes не заявляется. Stage 2 и owner/MMU вне scope; Mini и
performance остаются not_run до конца Stage 3. См.
[S3-33m report](../reports/STAGE_3_S3-33m_2026-10-30.md).

#### S3-33n. Composed exact native register profile

Add one separate opt-in, uncached `NativeRegisterV1` admission and eager /
demand-flags-candidate compile/dispatch path. Its accepted language is exactly
the union of the restricted S0-09 baseline, the already admitted bare NOP
interpreter fallback, and the exact extra forms committed in S3-33a–m:

- `48 FF C0` / `48 FF C8` / `48 F7 D8` / `48 F7 D0` (`INC` / `DEC` / `NEG` / `NOT RAX`);
- `48 83 C0 ib` / `48 83 F8 ib` / `48 83 E8 ib` (`ADD` / `CMP` / `SUB RAX,sign-extended imm8`);
- `48 C7 C0 id` (`MOV RAX,sign-extended imm32`), `48 87 D8` (`XCHG RAX,RBX`);
- `48 8D 03`, `48 8D 43 db`, `48 8D 04 0B`, and `48 8D 05 dd dd dd dd`
  (the four committed RAX LEA forms).

Use one complete-code decode/admission witness and one instruction-boundary
map; do not compose whole-program admission calls in series. Bind each admitted
extra to its existing exact predicate, source bytes, decoded operands and
signed displacement/immediate. Reject every other width, register, prefix,
memory data operation, encoding variant and any branch target in an instruction
interior across these variable lengths. Only a bare admitted NOP may route to
the interpreter. Lower each admitted form through its existing exact semantic
handler; leave every isolated S3-33a–m admission/API unchanged.

The targeted admission, mixed compiler/statepoint, and host parity gates have
passed. The five-instruction mixed compiler case remains below the existing
4096-byte emission cap and checks eager/candidate pending-ZF taken and
fallthrough MOV paths, STOP/budget boundaries, and bare-NOP fallback/control.
The host gate compares eager/candidate execution with the interpreter over
exactly 169 ordered pairs from 13 forms using seed `0x5333_336e_7265_6731`;
failure output includes seed, pair, bytes, and initial state. Harness
self-checks passed 28/28. Independent code/contract review was clean. The
post-review full local run recorded 73 passed, 0 failed, and 33 `not_run`
(`local_only`); all three S3-33n identities passed. This establishes the
bounded local evidence recorded in the report, not end-to-end Stage 3
acceptance. Mini, performance, Stage 2 and owner/MMU checks remain `not_run`
or outside this slice. See
[S3-33n report](../reports/STAGE_3_S3-33n_2026-10-30.md).

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

#### S3-34a. Двухоперационный IR memory differential corpus

Для двух уже принятых S3-20j/S3-20k последовательностей добавлен bounded
`od-host` integration corpus. Три фиксированных seed дают 2,400 уникальных
полных успешных входов (400 на последовательность и seed); каждый выполняется
дважды native entry и interpreter на независимых CPU/RAM копиях. Проверяются
полное состояние, RAM, exit/budget/retirement и порядок/число callbacks.
Входы варьируют qword state, alignment, совпадающие/разные ranges и безопасные
частичные overlaps там, где форма последовательности допускает два адреса.
Отдельные named случаи охватывают guest fault, STOP, budget и continuation;
comparator corruption check проверяет GPR, RIP, byte памяти, retirement и
пропущенный fault. Это локальное ARM64 evidence и частичный этап S3-34: reducer,
остальные native subsets, lifecycle suites и физическая проверка сюда не входят.
Физический S3-21 для этого corpus: `not_run`. См.
[S3-34a report](../reports/STAGE_3_S3-34a_2026-10-27.md).

#### S3-34b. Versioned replay of one saved two-site mismatch

The `od-host` corpus report now carries a strict version-1 case envelope with
the exact selected sequence/source bytes, initial supported CPU and private RAM
snapshots, instruction budget, STOP/stop-RIP request, typed site-0 fault
injection and trigger, and optional budget-then-continuation protocol. The
report records host/architecture/build revision, backend availability, and
result snapshots for each execution phase; continuation mismatch reports keep
the budget-limited prefix even when only the resumed phase differs. The ignored
`replay_saved_case_from_environment` test
requires `OD_JIT_REPLAY_CASE`; replay validates the report before publishing
native code, compares fresh native/interpreter results, and writes a new
create-only replay report beside the original. Unknown versions/profiles,
inconsistent source/state/addresses, malformed snapshots, unsupported protocol,
or missing native support are explicit failures. Round-trip, both successful
sequences, fault/STOP/budget/stop-RIP/continuation, comparator classes, and
malformed reports are covered locally; the physical S3-21 gate remains
`not_run`. См. [S3-34b report](../reports/STAGE_3_S3-34b_2026-10-27.md).

#### S3-34c. Bounded reducer for the two saved memory profiles

The ignored `reduce_saved_case_from_environment` entry uses
`OD_JIT_REDUCE_CASE` and accepts only S3-34b version-1 envelopes for the two
existing memory sequences. It first repeats the saved mismatch twice through
fresh native/interpreter execution; a nonreproducing mismatch stops with an
explicit report and zero reduction attempts. Candidate changes are limited to
non-address initial GPR values and private RAM bytes. Deterministic passes
reduce 8-byte groups, individual bytes, then bits, accepting a candidate only
when a simple popcount complexity strictly decreases and all original mismatch
classes remain phase by phase (including exact GPR/RAM indices); an identical
class moving from the first entry to continuation is rejected. The attempt limit is capped;
the saved result includes original/reduced envelopes, both initial replay
observations, reduced observations, classes, attempt count and stop reason.
The original file is create-only preserved, and a reduced envelope is checked
through ordinary S3-34b replay. A clearly marked input-dependent GPR[10]
corruption gated by nonzero GPR[11] is used only in reducer self-tests; it is
not evidence of a JIT defect. The generic reducer has no special handling for
the marker register and can reduce it when the mismatch predicate allows.
Mac mini and physical x86 validation remain
separate and `not_run`. См. [S3-34c report](../reports/STAGE_3_S3-34c_2026-10-27.md).

#### S3-34d. Проверка self-check для SMC lifecycle fixture

Существующий `TrackedCodeFixture` lifecycle scenario прогревает `MOV RDX,imm64`
и alias, затем committed code-store меняет immediate и доставляет notification.
Self-check сравнивает свежий native re-entry с interpreter snapshot по полному
CPU state и байтам всех backing RAM, проверяет изменение generation, удаление
обоих alias-кэшей prefix/tail и сохранение/reuse несвязанного entry. Контролируемая
подмена результата старым immediate должна классифицироваться как
`StaleCodeResult`; подмена backing generation — как
`BackingGenerationMismatch`. Проверка test-only и не отключает production
validation. Не добавлять общий lifecycle framework. См.
[S3-34d report](../reports/STAGE_3_S3-34d_2026-10-29.md).

#### S3-34e. Единый локальный correctness harness

Команда `python3 tests/cpu/stage3_harness.py --suite local` запускает
fail-closed локальный профиль: Stage 1 dispatcher сравнение (10 ручных + 1000
generated inputs, 2 повтора), двухоперационный memory corpus (2400 уникальных
inputs ×2, named fault/STOP/budget/continuation и comparator/replay/reducer
self-checks), затем exact host lifecycle tests для cache capacity/reuse,
bare-NOP fallback, code-region invalidation, V3 dirty-prefix/fault/STOP и
safe-slot lifetime. Каждый lifecycle test выбирается по точному Rust test
identity; zero-match и ignored обязательные случаи являются ошибкой.

Harness сохраняет immutable run directory под
`work/stage3-harness/<run-id>/`: summary, input manifest с SHA-256, команды и
логи; Stage 1 child summary и input manifest связываются своими digest. Он
фиксирует revision и исходный fingerprint дерева, сверяет его после каждого
шага и в конце, отклоняет
malformed/missing child evidence, timeout/signal/nonzero exit и частичный suite.
Self-tests требуют точные identities/count и успешный статус каждого теста;
нулевой запуск и все пропущенные тесты завершаются ошибкой. Parent input manifest
закрепляет Stage 1 source fingerprint и manual fixtures по `case_id`/SHA-256;
child должен совпасть с этим fingerprint обоими build revisions, а frozen inputs
и каждый manual result — с ожидаемыми digest и fixture path. Также проверяется
отказ при подмене input-manifest digest, неполном run и ложном `passed` для
невыбранных suites. Не ARM64 host завершается ошибкой; локальный результат
маркируется только `local_only`.

Mac mini/physical x86, performance, Stage 2 и owner-backed suites остаются
`not_run`. Эта команда не закрывает весь S3-34 или Stage 3. Фактический запуск
на 2026-10-29 и точные дочерние evidence указаны в
[S3-34e report](../reports/STAGE_3_S3-34e_2026-10-29.md).

#### S3-34f. Аудит точных local acceptance gates

Расширить input manifest и local run отдельными точными Cargo `--exact`
lanes для уже ограниченных S3-29e coordinator/demand-flags, S3-28 register
event/STOP boundary и S3-38a V0 ABI lifecycle/status сценариев. Для каждого
обязательного gate итоговый summary связывает имя шага, crate, полный Rust
test identity, точные argv, нулевой exit status и единственную строку
`test <identity> ... ok`. Отсутствующий gate, несовпадающий identity/argv,
нулевой match, ignored, timeout, nonzero exit или forged `passed` являются
ошибкой. Self-tests проверяют пропуск gate в manifest/summary и поддельное
свидетельство.

Матрица контрактов и границ приведена в
[S3-34f report](../reports/STAGE_3_S3-34f_2026-10-30.md). Process VMHost
regressions остаются отдельными от unit-level harness. Mini, performance,
physical x86, Stage 2 и owner-backed suites остаются `not_run`; полный S3-34,
S3-28, S3-29, S3-38 и Stage 3 не закрываются этим аудитом.

#### S3-34h. Добавить принятый exact LoadThenLoad в memory corpus

В bounded `od-host` corpus добавлен ровно уже принятый двухоперационный код
`48 8B 03; 48 8B 0B`. Три фиксированных seed дают 3,600 уникальных полных
успешных входов (400 на последовательность и seed), каждый сравнивается
native/interpreter дважды. Для нового профиля обе инструкции читают один и тот
же qword по RBX; трасса callback сохраняет операцию и адрес, чтобы сравнение
обнаруживало пропуск второго чтения и неверный адрес даже при совпадающих GPR.
Генератор покрывает выровненные и невыровненные смещения, оба края fixture RAM,
адреса 0 и 120 и промежуточные границы.

Version-1 envelope сохраняет прежний profile/version и прежние варианты. В уже
имеющемся `fault.site_index` принимаются site 0 и 1; operation, fault class,
linear address, sequence, source bytes, исходный CPU/RAM и layout проверяются
совместно. Отсутствующее поле `memory_accesses` в старых результатах по-прежнему
декодируется как пустая трасса. Site fault, budget, STOP и continuation
проверяются отдельно; continuation продолжает с committed snapshot и не
повторяет первое чтение. Reducer self-check фиксирует LTL, site-1 fault,
boundary address 120 и `gpr[10]` mismatch class на каждом кандидате.

Harness включает count 3,600, список обязательных Rust identities и отдельный
fail-closed Cargo gate
`reducer_preserves_load_pair_site_fault_boundary_and_mismatch_class` с точной
командой и результатом. Это bounded локальная проверка S3-34h, не закрывающая
весь S3-34 или Stage 3. Mini, Stage 2, owner-backed и performance проверки сюда
не входят. См. [S3-34h report](../reports/STAGE_3_S3-34h_2026-10-30.md).

#### S3-34i. Exact local gates for native arithmetic slices

The local harness requires nineteen exact Cargo gates for the committed
S3-33a–d native INC RAX, DEC RAX, NEG RAX, and ADD RAX imm8 slices, plus the
bounded S3-33f SUB RAX imm8 and S3-33g NOT RAX slices, and the S3-33h LEA
RAX,[RBX] slice. Each gate pins its
crate, full test identity, exact arguments, one executed passing test line, and
the relevant source files in the input manifest. Harness self-checks reject
missing gates, wrong package/identity/arguments, zero matches, ignored/failed/
timed-out tests, and forged passing evidence. The committed-tree local run on
`2a42c824` passed 40/40 steps, including all nine gates, with matching
start/end fingerprints. Final run metadata was added to the report after that
run, so the report's final contents are not part of the captured fingerprint;
the Rust and harness inputs remained unchanged. The run was `local_only`;
Mini/physical x86, performance, owner-backed, and Stage 2 suites were
`not_run`. This is a bounded extension and does not close S3-34 or Stage 3. See
[S3-34i report](../reports/STAGE_3_S3-34i_2026-10-30.md).

#### S3-34j. Add exact local gates for S3-33e CMP imm8

Add three mandatory exact Cargo gates for the committed `CMP RAX, sign-extended
imm8` profile: host full-state parity, eager/candidate compiler coverage, and
exhaustive exact admission. The manifest, planned steps, and final local
acceptance summary must contain all three identities exactly once, with the
expected crate, full test identity, exact argv, and one passing test outcome.
Self-checks must reject each omitted CMP gate from both the input manifest and
the final gate evidence. Existing fail-closed checks for incorrect package,
identity/argv, zero or duplicate matches, ignored/failed/timed-out tests, and
forged pass evidence continue to apply. The follow-up run and its limits are
recorded in the [S3-34j report](../reports/STAGE_3_S3-34j_2026-10-30.md); this
bounded slice does not close S3-34 or Stage 3.

#### S3-34k. Exact local gates for S3-33i XCHG RAX,RBX

Add four mandatory exact Cargo gates for exact bare XCHG RAX,RBX: admission,
dual-write emitter validation, eager/candidate compiler/statepoint coverage,
and host full-state/interpreter parity. Pin all four identities once in the
manifest, planned steps, and final local summary. Self-checks retain every
existing gate and reject an omitted XCHG gate, wrong identity/package/argv,
zero or duplicate matches, ignored/failed/timed-out results, and forged pass
evidence. The report records the full local suite after the sources freeze and
the independent review. Mini, performance, Stage 2, and owner-backed checks
remain outside this slice.

#### S3-34n. Reproducible generated corpus for NativeRegisterV1

Generate 1,200 unique complete inputs from three fixed seeds with 400 inputs
per seed. Run every input twice, from reset state, through the interpreter,
eager `NativeRegisterV1`, and the demand-flags candidate. Compare every modeled
CPU field, exit/RIP, retirement split and remaining budget; data memory must
remain untouched. Vary exact extra-form bytes and ordering, immediate and
displacement values, initial GPR/flags, guest base RIP, valid branch edges,
STOP/stop-RIP and budgets. Keep NOP fallback and baseline S0-09 forms in the
programs, and retain pending flag compositions, both branch outcomes, bounded
loops, STOP and budget exits.

Inputs have a strict versioned replay envelope containing source bytes, guest
base, full initial state (including explicitly defaulted auxiliary fields),
STOP/budget controls, profile, seed/index and canonical full-input digest.
Successful corpus execution saves a full case and its repeated backend
observations under the run's ignored artifact directory. The replay gate loads
the saved bytes/state and reruns both native backends. Mismatch diagnostics
retain the original case, both observations, and a direct reproduction command.
Comparator/envelope self-checks reject corrupted GPR, flags, RIP, exit and
retirement; malformed, altered-digest, wrong-profile or unsupported cases;
missing backend results; and backend errors/timeouts.

Add exactly three local acceptance gates and pin their full identities in the
manifest, source fingerprint and final summary while retaining all existing
gate identities. Do not add ISA/runtime/JIT behavior or a general reducer.
Report the exact seed, form coverage, counts and input digest. The full harness
is required only after review and source freeze; Mini remains deferred until
the end of Stage 3, while Stage 2, owner/MMU and performance are outside this
slice.

Completed 2026-10-30. The frozen local harness run
`stage3-20261030-234229-c0299d9b8f` finished `local_only` with 76 passed and
zero failed steps. It retained all 69 prior exact Rust gates, including all 42
S3-33 identities, and passed the three new gates. The corpus contained 1,200
unique inputs with canonical digest `fnv1a64:172fb17b01c502df`; the run source
fingerprints matched before and after. The result remains bounded local
evidence; Mini, physical x86, performance, Stage 2, and owner/MMU remain
`not_run` or outside scope. See the [S3-34n report](../reports/STAGE_3_S3-34n_2026-10-30.md)
for coverage counts, artifact hashes, exact gates, and review status.

#### S3-34o. Bounded value-only reducer for saved NativeRegisterV1 mismatches

The standalone reducer takes the path to an existing v1 `SavedCase`, validates
its profile/admission/digest and replays the original before attempting any
candidate. The original must reproduce the exact nonempty saved signature as a
stable semantic mismatch across both repeats of interpreter, eager, and
demand-candidate execution. Errors, missing observations, instability, and
non-semantic signatures reject the source or candidate. Every candidate is
re-digested, re-admitted and rerun twice through all three backends; acceptance
requires exact whole-signature preservation and a strict decrease in the
explicit metric (popcount of initial GPRs, mutable arithmetic flags/DF, and
allowed instruction immediate/displacement values).

Only initial GPR, arithmetic flags/DF (`0x0cd5`, retaining all other and
mandatory RFLAGS bits), and existing immediate/displacement value bytes are
candidate fields. Instruction boundaries are decoded before locating eligible
fields. Branch displacement bytes and targets, opcodes/order/lengths, base and
entry RIP, auxiliary state, STOP controls and budget are immutable. No
instruction is deleted and no relocation is performed: this is value
reduction, not instruction minimization. Candidate ordering and exact-byte
deduplication are deterministic; evaluation stops at 256 candidates. Reaching
the cap reports `evaluation_limit` and does not claim global minimality. No
reduced envelope is emitted unless the original mismatch reproduces and at
least one strictly simpler candidate preserves its exact signature.

Artifacts are written create-only under the harness run directory: the
unchanged source envelope, report with source and candidate backend observations
for every repeat, exact signatures, attempt counts, before/after metric, stop
reason and source/reduced replay commands; a reduced envelope and digest exist
only after an accepted candidate. The positive self-check deliberately injects
a local observation mismatch to exercise reducer mechanics; it is explicitly
labelled as self-check data and is not replay or JIT defect evidence. Two exact
local gates are added, taking the committed Rust gate inventory from 72 to 74
while retaining all 42 S3-33 identities and the 3 S3-34n gates. Mini remains
deferred until the end of Stage 3; physical x86, Stage 2, owner/MMU and
performance are outside this slice. This bounded value reducer neither proves
global minimality nor completes S3-34 or Stage 3.

Focused checks and full local harness evidence are recorded in the
[S3-34o report](../reports/STAGE_3_S3-34o_2026-10-30.md).
Implemented and reviewed 2026-10-31. The frozen local harness run
`stage3-20261031-004600-2e6dd54a75` passed all 78 local steps with matching
source fingerprints; the exact Rust-gate inventory is 74. The result remains a
bounded local slice only, with Mini deferred to the end of Stage 3 and physical
x86, performance, Stage 2 and owner-backed checks `not_run`.
После зафиксированного прогона изменились только этот план и S3-34o отчёт;
Rust- и harness-исходники не менялись. Записанный fingerprint относится к
проверенному до этих документальных правок срезу исходников, а не к финальному
состоянию документации.

#### S3-34p. Reproducible cached NativeRegisterV1 corpus and replay

Reuse the exact 1,200 generated inputs and interpreter/eager/candidate
observations saved by S3-34n. Preserve the S3-34n `SavedCase` v1 format and
reducer behavior. A separate versioned cache-trace envelope records exact
inputs, full baseline and cached observations, strategy, execution-context and
code-dependency bindings, ordered sequence indices, and cumulative
single-entry cache counters. It contains a fresh-cache corpus sequence for
eager and demand-flags candidate, plus one explicit retained one-block
ADD/RAX sequence per strategy. The latter changes incoming RAX and RFLAGS
between calls and requires an actual cache hit with no additional compilation
or publication. Do not infer hit-only behavior for the generated multi-block
corpus; preserve the observed counters for each input.

Replay validates the envelope version/profile, digest, full input bindings,
source corpus order, state/exit/retirement/budget parity, and exact counters,
then runs every ordered segment from a new empty cache. Corrupted inputs,
counter changes, missing steps, unknown fields, and altered digests are
rejected. A cached mismatch in either the generated or retained-hit segment
writes a separate versioned failure envelope with the successful prefix,
failing input, full observations and before/after counters. Its bounded reader
verifies source identity and digest, replays each prefix step and the failing
input, and rejects changed or unreproduced outcomes; the gate's explicit local
fault injection covers save/read/replay for both segment kinds without
claiming a JIT defect. The host uses the real cached executor and releases each
cache's resident published handle before ending its segment. Three exact Rust
gates and Python harness self-checks pin the corpus, replay and failure replay
identities. Producer and consumer bind the source observations artifact path,
SHA-256 and recomputed envelope digest, while preserving all prior gate
identities and the explicit retained hit. This is bounded local correctness/replay evidence only; it changes no
production API, ISA, cache policy, ABI, or default. Mini, physical x86,
performance, Stage 2 and owner/MMU remain deferred/outside the slice.

Implementation and final local evidence: [S3-34p report](../reports/STAGE_3_S3-34p_2026-10-31.md).

#### S3-34p3. Обязательный local gate для S3-30p3

В обязательную inventory local harness добавлен один exact gate для уже
существующего test-only probe `tests::allocated_register_and_spill_values_survive_helper_clobbers`.
Проверены точные identity/argv/log binding и fail-closed реакции на отсутствие
gate, zero-match, ignored, подмену identity/argv/log и forged success; p4 gate
остался без изменений и встречается ровно один раз. В run
`stage3-20261101-111248-72db14afde` команда harness с `--suite local`
завершилась с exit 0: 102 шага
passed, failures — 0; обязательный Rust inventory содержит 98 gates. P3 exact
test прошёл 1/1. Source fingerprint до и после run одинаковый:
`8843203a2470cccc21b22c6590f95ec14432364eb369c5853d04de9a87f9c878`.
Это только bounded local harness acceptance; Mini, physical x86, VMHost
process, performance, owner-backed и Stage 2 в этом run — `not_run`. S3-34 и
Stage 3 остаются открытыми. Факты и локальные artifact bindings приведены в
[S3-34p3 report](../reports/STAGE_3_S3-34p3_2026-11-01.md).

#### S3-34 local gates для S3-32a

В mandatory local inventory добавлены три exact gates для уже существующих
S3-32a проверок: lowering точного кандидата, проход через обычный verified
compile pipeline и parity с interpreter на краевых значениях и budget resume.
Инвентарь содержит 101 Rust gate: 91 acceptance и 10 lifecycle. Self-check
проверяет точные identity/package/argv/log/source bindings, обязательность
каждого gate и отказ на zero-match, ignored output, подмену параметров или
forge success.

Полный `--suite local` run `stage3-20261101-142811-25240b316e` завершился с
`local_only`: 105 шагов `passed`, 0 failures, 33 шага `not_run`. Все три новых
S3-32a tests прошли по одному разу. Source fingerprint до и после совпал:
`fc13385ae96175d9745071afb080165c1e1bfc8156b1d9590611d9a0bdb6b73d`; summary
фиксирует revision `fdb744b463990ac77f5e8c8a744966a8513eabec`. Поздний commit
`de98d9f` содержит тот же проверенный harness source. Дочерний Stage1 dispatch
имеет scope `local_only`; VMHost process, physical x86, Mac mini, performance,
owner-backed и Stage2 suites имеют статус `not_run`. Проверки на Mac mini
отложены до конца Stage 3. Это локальное bounded evidence; S3-34 и Stage 3
остаются открытыми. Summary, input manifest, logs и corpus artifacts сохранены
в [S3-34 local report data](../reports/data/stage3/s3-34-local/) и связаны в
[отчёте](../reports/STAGE_3_S3-34-local_2026-11-01.md).

### S3-35. Измерить холодное исполнение

#### S3-35a: first full-corpus interpreter/FIFO comparison path

Implemented opt-in `od-bench cold-first-run` for the frozen S3-17
register-only corpus. It performs correctness preflight outside timed samples,
alternates interpreter and default FIFO(16) block-dispatcher samples, and
creates a fresh runtime/cache/arena for every cold dispatcher sweep. Timed
scope includes initialization and first corpus execution; setup of fixture
inputs, state reset/validation and teardown are explicitly excluded. Raw
sample reporting and host/build/input fingerprints are implemented. Three
release process runs completed on the frozen corpus after two initial
calibration attempts ended below the 200 ms floor; their logs are linked in
the S3-35a report. The cold FIFO sweep median was 6.64–6.69 ms per 1,010-case
corpus sweep versus 0.164–0.166 ms for the interpreter. This describes one
bounded cold workload and does not support a general speed claim. Separate
pipeline component timings remain open, so S3-35 is partial. Legacy fragment
timing is not applicable unless an admitted legacy input shape and semantics
are established for these same inputs. See the
[S3-35a report](../reports/STAGE_3_S3-35a_2026-10-30.md) and
[`BENCHMARKS.md`](../testing/BENCHMARKS.md#s3-35a-cold-first-full-corpus-execution).

1. Измерить полный первый запуск при пустом code cache: необходимые decode/IR/lowering/allocation/publication/lookup/execution, с явно указанной стоимостью initialization.
2. Отдельно снять составляющие pipeline, не складывая несовместимые independently calibrated samples как точный end-to-end результат.
3. Для каждого cold-cache sample гарантировать отсутствие готового block; повторное использование process/runtime ресурсов явно обозначить. Cold process/VMHost launch измерять отдельно, если он входит в scenario.
4. Сравнить с interpreter и прежним fragment path там, где inputs/semantics сопоставимы. Сохранить расходы на compile и overhead fallback.

**Готово:** cold результат не замаскирован warm reuse. Замедление первого запуска допустимо как измеренный факт и должно быть отражено; обещание ускорения первого исполнения без данных запрещено.

#### S3-35b: cold FIFO pipeline components

На том же зафиксированном S3-17 corpus/trace и production FIFO16 path отдельно
измерить block build/admission, key/snapshot/lookup, IR lowering, allocation,
emission/encoding/finalization, publication и host-execution boundary. Cache hit
по-прежнему оплачивает build и lookup. Guards и install/accounting оставить в
остатке полного dispatch interval. Сравнить ordinary no-op и observed
dispatcher при одинаковой работе, проверив полное per-case состояние/exit,
retirement и все cache counters. Сохранить границы initialization/teardown,
raw paired samples, hashes и timer/observer overhead; минимум 20 пар по 200 ms
в каждом sample и три последовательных release process run. Не складывать
несовместимые component samples как точный end-to-end результат и не выбирать
оптимизацию/default по этому срезу.

Source review, targeted checks и три последовательных release process run
завершены. Frozen S3-17 corpus прошёл normalized full-state/exit/budget/
retirement parity; paired observed/ordinary ratio median составил 1.0301 по 60
парам. Начальная серия была исключена после обнаружения Stage 2 test в том
окне; процесс не останавливали, а чистую серию повторили после его завершения.
Подробные интервалы, spread и raw hashes: [S3-35b report](../reports/STAGE_3_S3-35b_2026-10-31.md).
Mini не запускался, Stage 2 не менялся. Этот результат не выбирает оптимизацию
или default.

#### S3-35c: cold direct-entry checked-memory profile

Три release process run завершили opt-in `od-bench two-site-memory-cold` на
точно двух замороженных S3-34a sequences × 3 фиксированных seed × 400 inputs
(2,400 уникальных inputs; третий sequence не включён). Каждый input прошёл
две preflight trials на каждом backend: 4,800 interpreter и 4,800 native
executions, полное CPU/RAM/exit/retirement/budget сравнение и точная ordered
callback trace. Native timed interval включает compile IR, publish, direct
typed entry, explicit drop `PublishedCode` и IR; interpreter interval включает
выполнение interpreter. Reset и validation находятся вне обоих интервалов.
Dispatcher, VMHost и cache reuse не используются.

В каждом процессе сохранены 20 alternating timing pairs с 1,024 repetitions
на сторону; все 60 пар прошли floor 200 ms без trimming. Median raw-total ratio
`native_total_ns / interpreter_total_ns` по 60 парам — 71.4765, range
66.7736–93.9423. Эти числа относятся только к этому cold checked-memory
profile и не выбирают production default или общий winner. В каждом процессе
наблюдались Stage 2 snapshots; run 1 и run 3 также имеют Graphify activity
snapshots. Нет pair timestamps, поэтому per-pair overlap и его влияние
неизвестны. Для run1/run2 numeric executor exit недоступен и выводится из
scored report + all-floor gate; run3 exit0 записан явно. Runtime dirty status
неизвестен (`null`) из-за git metadata timeout. Подробнее и все ограничения:
[S3-35c report](../reports/STAGE_3_S3-35c_2026-11-01.md),
[evidence archive](../reports/data/stage3/s3-35c/).

Нижний calibration cap дал диагностический результат без interpreter floor;
эта попытка сохранена. Cap 2048 отдельно прошёл calibration-only при 1,024
repetitions. Проекция 16.7555 min включает cumulative calibration до reserve cap 2,048 и
20 пар при 2,048. Это линейная оценка, не измеренный full-process runtime;
setup, teardown и другой overhead исключены. Scored runs остановились на 1,024. S3-35 остаётся partial:
третий sequence, dispatcher/VMHost и broader checked-memory workloads не
измерялись. Mini отложен до конца Stage 3.

#### S3-35d. Сравнение свежих процессов на общих register-only входах

Один release `od-runner` прогнал пять закреплённых S3-14 branch/helper
фикстур через interpreter, legacy fragment JIT и uncached IR dispatcher.
В каждом из трёх прогонов собрано по 24 группы с балансировкой порядка
движков и одинаковыми 32 sweep на sample. Все 34 560 измеренных запусков
совпали по результату с локальным интерпретатором. Медианы времени
на пять процессов составили 14.073–15.459 мс для интерпретатора,
14.082–15.277 мс для legacy JIT и 14.277–15.587 мс для IR dispatcher.
Парные диапазоны пересекли равенство. Сравнение включает запуск процесса и
обработку JSON; разницу времени самого JIT оно не изолирует.
Команда и [сырые замеры](../testing/BENCHMARKS.md#s3-35d-fresh-runner-process-comparison)
сохранены. Общая приёмка S3-35 остаётся открытой.

#### S3-35e. Первый RUN нового S0-09 Session

Все 576 измеренных сторон прошли проверку на одинаковом гостевом объёме
работы. Каждый вызов JIT начал с пустого кеша. Таймер включает создание
`Session` и первый RUN. На одном RIP текущая сборка уложилась в 10,6 мкс
против 27,5 мкс у интерпретатора при 64 итерациях и в 163 мкс против
1752 мкс при 4096 итерациях. Короткие и двух-RIP сценарии остались
медленнее интерпретатора. Полная матрица S3-35 остаётся открытой.
[Отчёт и сырые данные S3-35e](../reports/STAGE_3_S3-35e_2026-11-19.md)

#### S3-35f. Первый RUN после упрощения поиска в кеше

Сравнение `06bb39a` и `2b0f598` повторило шесть S3-35e профилей
с пустым кешем. Все 768 измеренных сторон прошли аудит. После
нормализации относительно интерпретатора новый JIT быстрее предыдущего
во всех четырёх раундах для `same_4096`, `two_64` и `two_4096`.
В остальных профилях диапазон включает 1. Текущий JIT быстрее
интерпретатора на одном RIP при 64 и 4096 итерациях во всех парах.
Оба профиля с двумя RIP и короткие циклы быстрее на интерпретаторе.
Полная матрица S3-35 и отдельные фазы компиляции остаются открытыми.
[Отчёт и сырые данные S3-35f](../reports/STAGE_3_S3-35f_2026-11-19.md)

#### S3-35g. Первый запуск смешанного регистрового пути

На четырёх входах S3-55 измерено создание допуска, FIFO16 cache и
исполняемой арены вместе с первым RUN. Два прямолинейных потока и два
цикла исполняют native blocks и точные CLD/STD interpreter steps.
В каждой паре холодный JIT медленнее интерпретатора. Результаты, raw
samples и раздельные счётчики опубликованы вместе с замером warm.
[Отчёт S3-35g/S3-36i](../reports/STAGE_3_S3-35g_S3-36i_2026-11-19.md)

#### S3-35h. Холодный вызов проверяемого кеша памяти

Для обоих профилей S3-34a измерен полный `MemoryPairCache::run` с
пустым кешем. В каждом вызове проверены одна компиляция и публикация,
полное состояние CPU, RAM и порядок обращений. Медиана текущей сборки
составила около 7,89 мкс на вызов в обоих профилях. Подготовка входа и освобождение
публикации проходили вне таймера.
[Парные результаты и сырые данные](../reports/STAGE_3_S3-35h_S3-36k_2026-11-19.md)

### S3-36. Измерить прогретое исполнение

#### S3-36a: bounded resident warm register-only path

Implemented opt-in `od-bench warm-first-run` for a deterministic prefix of the
frozen S3-17 register-only corpus whose conservative sum of per-case
`unique_entries` fits FIFO(16); shared keys may make the actual resident set
smaller. The report records the full corpus fingerprints and selected case
IDs. A complete 1,010-case sweep cannot serve as a resident warm sample: the
FIFO cache holds 16 entries and the collected cold corpus sweep observed 1,320
misses/compiles and only 2 hits. The runner prewarms one persistent runtime,
verifies that measured sweeps have hits and zero
miss/compile/publication/eviction deltas, alternates interpreter and cached
dispatcher samples, and validates each reset execution outside the timed
calls. Three release runs measured 12.129–12.343 µs per warm FIFO sweep and
2.019–2.053 µs per interpreter sweep on this slice. The interpreter was about
6.0× faster for this workload; no general performance conclusion is supported.
It covers register-only cases. The narrow accepted-fixture branch/helper
profile is recorded separately in S3-36b; memory workloads and the full S3-36
matrix remain open. See the [S3-36a report](../reports/STAGE_3_S3-36a_2026-10-30.md).

#### S3-36b: bounded S3-14 branch/helper warm profile

Added the explicitly named `od-bench warm-branch-helper` profile over the
fixed five-case list of accepted S3-14 conditional-branch and live-through
helper fixtures. It is bound to exact case-list and recorded-trace SHA-256
values, validates the recorded interpreter/JIT outcomes and retirements before
timing, selects a deterministic resident subset that fits FIFO(16), warms one
retained FIFO runtime, and admits only hit-only timed FIFO samples. Calibrated
interpreter/dispatcher samples alternate, with raw durations, counters,
timer-scope metadata, and input hashes in the report. The earlier
`warm-first-run` remains bound to the full S3-17 register-only corpus and keeps
its existing behavior. Three release runs on the five-case profile measured
FIFO at 3.736–3.959 µs/sweep and interpreter at 0.726–0.751 µs/sweep;
interpreter was about 5.3× faster on these fixtures. This is narrow
branch/helper evidence only and does not cover memory workloads or close the
full S3-36 matrix. See the
[S3-36b report](../reports/STAGE_3_S3-36b_2026-10-30.md).

#### S3-36c: resident warm NOP-fallback comparison

Added opt-in `od-bench warm-native-fallback` for 19 exact seed/index inputs
from the frozen S3-34p corpus. Selection is fixed before timing and verified
again by requiring `BudgetExhausted`, one interpreter-retired NOP immediately
before the oracle's final RIP (later code bytes can be unexecuted), two or
three native retirements, and exactly one eager repeat cache hit with zero
misses, compiles, publications, or replacements. The profile separates
the ten two-native-plus-fallback cases from the nine three-native-plus-fallback
cases. Each case retains one capacity-one eager cache from one untimed prewarm
through calibrated alternating interpreter/cached samples, then drops that
mapping before the next case. Every timed cached call must add exactly one hit
and no other cache activity; state, exit, budget, and retirement parity are
checked outside each timed interval. Implementation and CLI wiring are in
place. Three sequential release runs on M3 Max completed with 20 retained
alternating pairs per input. The run-level median-of-case-medians ranged from
5.223 to 5.259 for cached/interpreter elapsed ratios in the two-native stratum
and from 4.313 to 4.376 in the three-native stratum. These results apply only
to the frozen native-plus-NOP `BudgetExhausted` inputs; intermittent Stage 2
compiler/test activity was observed, and a brief Graphify process overlapped
run 3. Point-in-time process
snapshots have no per-pair timestamps, so the timing effect of overlap cannot
be assigned to samples. This does not measure general fallback frequency,
memory workloads, or whole-system performance. S3-36 and S3-41 remain open.
See the [S3-36c report](../reports/STAGE_3_S3-36c_2026-10-31.md) and
[`BENCHMARKS.md`](../testing/BENCHMARKS.md#s3-36c-resident-warm-nop-fallback-comparison).

#### S3-36d: resident warm direct-entry checked-memory profile — partial

Implemented opt-in `od-bench two-site-memory-warm` for only the two original
S3-34a sequences, three fixed seeds, and 400 inputs per seed/sequence (1,200
per sequence; 2,400 unique inputs). The profile compiles and publishes one
block per sequence, retains that mapping through preflight, warmup, calibration
and samples, then drops it before compiling the next sequence. The timed native
path is direct typed entry; interpreter execution is measured separately.
Preflight uses two trials per backend/input and checks full CPU/RAM/exit/budget
and callback parity. The separate calibration-only run passed at 2,048
repetitions per side. Three scored runs retained 20 pairs per sequence and all
sides passed the 200 ms floor. The pooled raw-total native/interpreter ratio
median/range was 1.3718847445 [1.3116083380, 1.4203506060] for
`store_then_load` and 1.3662312961 [1.2865399535, 1.4421564464] for
`load_then_store`; the two strata are not combined.

This is partial warm direct-entry checked-memory evidence only. It has no
dispatcher/lookup or cache counters and does not cover the third sequence,
other memory forms, owner/MMU, VMHost, SMC, or broader guest workloads. Stage 2
and Graphify records are point-in-time snapshots; there are no per-pair
timestamps, and runtime dirty status is unknown. Mini was not run and remains
deferred to the end of Stage 3. Full S3-36 dispatcher/lookup and workload
coverage remains open. See the [S3-36d report](../reports/STAGE_3_S3-36d_2026-11-01.md)
and [evidence archive](../reports/data/stage3/s3-36d/).

1. Прогреть cache вне timed samples и подтвердить hit/compile counters. Повторять одинаковую гостевую работу с корректным reset initial state.
2. Измерять полный steady-state runtime path с lookup/dispatcher/helpers/flags и exits, а не только одну ARM64 arithmetic instruction.
3. Чередовать варианты, применять повторные calibrated samples, сохранять raw elapsed/work counts, median и разброс; при пересекающемся шуме не утверждать speedup.
4. Отдельно показывать branch-heavy, helper-heavy и memory workloads, долю fallback, code size и resources. При осмысленности вычислить break-even числа повторов с учётом cold cost.

**Готово:** ускорение или его отсутствие установлено для конкретных workloads. Если подтверждённого ускорения целевого прогретого сценария нет, correctness slice может быть принят отдельно, а performance-критерий этапа остаётся незакрытым.

#### S3-36e: resident capacity-one cached checked-memory full-call profile — bounded profile complete

Added opt-in `od-bench two-site-memory-cache` for the same two frozen S3-34a
sequences (`store_then_load`, `load_then_store`; 1,200 inputs each). This profile
retains one capacity-one `MemoryPairCache` and one mapping per sequence. Its
timed native call includes profile/context/source/dependency/key validation,
cache lookup/counters, and typed host execution; interpreter execution is
measured separately. The release preflight passed on Apple M3 Max/aarch64:
2,400 unique inputs, two trials per backend/input, 4,800 executions per
backend and parity trials, with one initial miss/compile/publication followed
by 2,399 hits per sequence and one resident entry. No timing started during
preflight. A separate calibration-only release run passed the 200 ms floor
for both sequences at 2,048 repetitions per side, retained 12 attempts per
sequence, and emitted no scored pairs. Runtime `working_tree_dirty` is unknown
because the Git status probe timed out; build source tree was clean. All three
scored runs completed with 20 retained pairs per sequence and all sides above
the floor. Across 60 pairs, the median raw native/interpreter ratios were
2.7223 (range 2.6348–2.8856) for `store_then_load` and 2.7650
(2.6390–2.9279) for `load_then_store`; the strata remain separate. This is a
bounded fixture result, not general memory
or production owner/MMU evidence. Mini checks remain deferred to the end of
Stage 3. See the [S3-36e report](../reports/STAGE_3_S3-36e_2026-11-01.md),
[preflight evidence](../reports/data/stage3/s3-36e/preflight-release-run1.json),
the [calibration evidence](../reports/data/stage3/s3-36e/calibration-run1.json),
[scored runs 1](../reports/data/stage3/s3-36e/run1.json),
[2](../reports/data/stage3/s3-36e/run2.json), and
[3](../reports/data/stage3/s3-36e/run3.json), with the independently reviewed
[suite manifest](../reports/data/stage3/s3-36e/suite-manifest.json).

#### S3-36f: cached dispatcher register recurrence comparison — bounded profile complete

Added the opt-in `od-bench cached-native-loop` profile for two fixed
helper-free `NativeRegisterV1` recurrences: three instructions per iteration
(`LEA; XCHG; JNZ`) and five instructions per iteration (two `LEA; XCHG`
pairs plus `JNZ`), each at 1, 64, and 4,096 iterations. Release preflight
passed all six strata on Apple M3 Max/aarch64/macOS 26.7.1; it checked zero
work, exact budget, the final in-loop instruction budget boundary, internal
`StopAddress`, full CPU/flags/RIP/retirement parity, the independent wrapping
recurrence, a single resident publication, and code/logical cache sizes.
Preflight did not time samples. Three independent release processes retained
20 alternating pairs per stratum each, with every side above 200 ms. Across 60
pairs, native/interpreter median ratios and full paired ranges were:

| Stratum | Median ratio | Full range |
|---|---:|---:|
| `a_1` | 4.7661 | 4.4800–5.1930 |
| `a_64` | 4.8890 | 4.4135–5.1931 |
| `a_4096` | 4.8938 | 4.6885–5.2546 |
| `b_1` | 3.7205 | 3.5583–3.9703 |
| `b_64` | 3.8092 | 3.2670–4.0133 |
| `b_4096` | 3.7805 | 3.4622–4.0340 |

Every observed ratio was above parity, so the interpreter was faster on this
synthetic register workload. The profile does not establish that native JIT
execution is useful for a real guest workload, does not justify changing the
cache or runtime default, and does not close S3-36/S3-40/S3-41. The report
records first full dispatch (including compile/publication/execution) and
resident code/logical bytes separately; these values are not decomposed into
compile-only cost and are not used to infer break-even. The build source tree
was clean at revision `a5b368eab1d1cdfa3e47a28d0d7c1263b12978c7`; runtime Git
dirty status is `null` because the two-second status probe timed out. Stage 2
activity did not gate the runs. Mini checks remain deferred to the end of
Stage 3. See the [S3-36f report](../reports/STAGE_3_S3-36f_2026-11-01.md),
[release preflight](../reports/data/stage3/s3-36f-20261101/preflight-release.json),
[process 1](../reports/data/stage3/s3-36f-20261101/process-1.json),
[process 2](../reports/data/stage3/s3-36f-20261101/process-2.json),
[process 3](../reports/data/stage3/s3-36f-20261101/process-3.json), and the
[suite manifest](../reports/data/stage3/s3-36f-20261101/suite-manifest.json).

#### S3-36g. Повторный RIP в прогретом retained Session

S0-09 dispatcher повторно использует индекс последней записи кеша внутри
одного вызова при возврате на тот же RIP. Три парных release-раунда на
штатном Rust `Session::run` показали, что при 64 и 4096 итерациях медиана
времени стала 7,6% и 5,6% от предыдущего JIT. Один проход стал дороже на
3,75% по медиане абсолютного сравнения. Сценарии с двумя RIP сохранили
обычный поиск и дали медианный нормированный overhead 2,22–3,47%. Полный
набор прогретых сценариев и физические проверки остаются открытыми.
[Отчёт и сырые данные S3-36g](../reports/STAGE_3_S3-36g_2026-11-19.md)

#### S3-36h. Один поиск на обычное попадание в кеш S0-09

Dispatcher использует индекс после `record_hit` и запоминает его только
при продолжении с тем же RIP. Все 576 измеренных сторон прошли проверку
объёма работы. Медиана абсолютного времени одного прохода снизилась на
3,73%, медианы двух-RIP случаев после нормировки — на 2,82% и 3,02%.
При двух RIP интерпретатор по-прежнему быстрее. Общая приёмка S3-36 открыта.
[Отчёт и сырые данные S3-36h](../reports/STAGE_3_S3-36h_2026-11-19.md)

#### S3-36i. Прогретый смешанный путь с CLD и STD

Четыре S3-35g профиля повторены с резидентным кешем на той же сборке
и с теми же входами и объёмом исполнения. Все native entries дают hits.
На циклах в 64 и 512 итераций интервалы прогретого JIT примерно в 2,10
раза длиннее интерпретатора по медианам пар. Два прямолинейных потока
также медленнее интерпретатора в каждой паре. Общая матрица памяти и
других fallback профилей остаётся открытой.
[Отчёт S3-35g/S3-36i](../reports/STAGE_3_S3-35g_S3-36i_2026-11-19.md)

#### S3-36j. Один поиск на попадание в кеш NativeRegisterV1

Диспетчер получает актуальную позицию записи из обработчика попадания,
в том числе после перемещения записи в LRU. В четырёх смешанных FIFO
профилях с eager flags проверено то же состояние и тот же объём работы.
После нормировки медианы двух прогретых прямолинейных профилей снизились
на 1,86% и 2,58%. В обоих случаях выигрыш повторился во всех раундах.
На циклах прогретые медианы также снизились, а диапазоны пересекли
паритет. Интерпретатор оставался быстрее во всех зачётных парах.
[Отчёт с холодными результатами и сырыми данными](../reports/STAGE_3_S3-36j_2026-11-19.md)

#### S3-36k. Прогретый вызов того же кеша памяти

Парный прогон использует те же входы, таймер и сборку, что S3-35h.
На текущей сборке медианное отношение прогретого вызова к интерпретатору
составило 0,597 и 0,602. Все тройки дают выигрыш после публикации.
При повторе на исторической сборке `d24865d` интерпретатор быстрее во
всех тройках. Причина различия времени интерпретатора открыта в ISSUE-521.
[Отчёт и границы замеров](../reports/STAGE_3_S3-35h_S3-36k_2026-11-19.md)

### S3-37. Проверить cache churn и ресурсы

#### S3-37a: bounded FIFO16 corpus churn observation path

Implemented opt-in `od-bench cache-churn` for exactly 128 complete sweeps of
the frozen S3-17 register-only corpus, using one retained production FIFO16
runtime/cache/arena. It records per-sweep and cumulative cache hit/miss,
compile/publication, and eviction counters, plus resident entry/code/logical
metadata high-water and arena reservation plus a derived unoccupied-slot value
(`capacity - resident entry high-water`). The latter is not read from an
allocator availability counter. The
runner requires observable eviction/recompile and uses the existing 30-minute
limit. Three release processes each completed 128 sweeps with resident
high-water at 16 entries, 21,640 code bytes, and 50,124 logical metadata bytes;
each recorded 168,944 evictions and 168,960 successful compiles/publications.
This is bounded FIFO churn evidence, not a leak-free stress result. Process RSS,
runtime teardown, independent-instance lifecycle, SMC invalidation, and broader
workloads remain unmeasured. See the
[S3-37a report](../reports/STAGE_3_S3-37a_2026-10-30.md).

#### S3-37b: process-wide RSS snapshots for bounded FIFO16 churn

The `cache-churn` report now takes process-wide RSS snapshots after corpus
preparation but before FIFO runtime creation, after the fixed sweeps but before
runtime drop, and after explicit runtime drop. On macOS it reads `/bin/ps` RSS
in KiB and converts to bytes. Other targets and sampling failures are reported
as `null` with a reason. Three release processes completed the same 128-sweep
corpus: pre-runtime RSS was 5,024–5,056 KiB, pre-drop RSS was 5,808–6,000 KiB,
and post-drop RSS was 5,744–5,936 KiB. Post-drop snapshots were 64 KiB below
the pre-drop snapshots in all three processes, while remaining 688–912 KiB
above each process's pre-runtime baseline. These process-wide values include
the runner and its allocations; allocator retention and process-wide RSS do
not establish cache attribution, leak-free behavior, or per-runtime release.
See the [S3-37b report](../reports/STAGE_3_S3-37b_2026-10-30.md) and
[benchmark protocol](../testing/BENCHMARKS.md#s3-37b-process-wide-rss-snapshots).

#### S3-37c: deterministic invalidation and arena lifecycle path

Added opt-in `od-bench cache-lifecycle` over 17 admitted one-entry fixtures
from the frozen S3-17 corpus. Its fixed sequence observes FIFO16 fill, a
controlled stop-RIP boundary returning `StopAddress` (distinct from async
`StopRequested`) with no dispatch/lookup/cache clear, repeated hit,
replacement, supported between-call backing invalidation, changed-code
recompile and hit, execution-context generation advance, clear, runtime drop,
and fresh runtime reopen. Every guest dispatch checks interpreter parity. Snapshots
read current resident code/metadata accounting and allocator available slots
directly, keeping high-water counters separate. This path does not exercise
production guest SMC, STOP clearing, concurrent arenas, faults, or leak-free
stress. Three sequential release runs passed and the independent source and
evidence reviews were clean; see the
[S3-37c report](../reports/STAGE_3_S3-37c_2026-10-31.md). The
[protocol](../testing/BENCHMARKS.md#s3-37c-deterministic-fifo16-invalidation-and-runtime-lifecycle)
defines reproduction and limitations. Full S3-37 and its long-running stress
criteria remain open.

#### S3-37d: sustained deterministic mixed lifecycle stress

Add a separate versioned `od-bench cache-lifecycle-stress` profile over the
same frozen corpus and existing FIFO16 runtime. V5 calibration uses 512 warm-up
epochs followed by eight sustained 65,536-epoch measurement batches on one
retained runtime; it sizes from the fastest observed batch and keeps the 128
bounded progress checkpoints distinct from scalable epoch-body time. The
scored count targets at least 11 minutes, reserves one minute below the
30-minute process limit, and is capped at 50 million epochs. Calibration binds
the source fingerprint, host/build, corpus, warm-up/batch/checkpoint schedule
and validated timing arithmetic. A scored artifact records whether actual
elapsed time met 660 seconds; an under-target result is persisted and the
command fails. One overall deadline covers preparation through final report
persistence. Each epoch performs a current-key hit, a
unique pressure miss and one FIFO eviction, between-call invalidation of the
current backing, alternate admitted code under the advanced backing version
with a distinct interpreter result, and a verified next-call hit. Every
dispatch checks full interpreter-visible state, exit, budget, and retirement.
Periodic clear must report zero current resident accounting and 16 free slots;
periodic teardown/reopen must execute successfully with one MAP_JIT arena at a
time. Current occupancy remains separate from high-water values, and progress
is bounded by at most 128 aggregate checkpoints. RSS is auxiliary only.

Three sequential v5 scored processes completed in Stage2-quiet windows with
exact counters and met the duration target: 671.161073750, 679.267265042, and
688.093795250 seconds. Source, current-HEAD calibration, and each scored report
passed independent review. Run1 used the earlier accepted v5 calibration; runs
2 and 3 use calibration bound to HEAD e5c149b. V4 run1 completed 29,682,315
epochs in 439.482 seconds, below target; it remains separate lifecycle/counter
diagnostic evidence. Earlier overlapping attempts remain inconclusive and
excluded. See the [S3-37d report](../reports/STAGE_3_S3-37d_2026-10-31.md)
and its raw data. This supported-API resource stress does not claim production
SMC/owner notification, faults, concurrent arenas, SMP, or leak-free behavior.
Mini remains deferred to the end of Stage 3; owner/MMU and Stage 2 are outside
this slice.

1. Выполнить длинную серию unique blocks, повторных invalidations, capacity evictions, SMC и mixed hit/miss workloads.
2. Снимать executable bytes, metadata bytes, live/retired blocks, compile counts и доступные process memory показатели с описанием метода.
3. Проверить освобождение после STOP/fault/destroy и несколько независимых runtime instances. Не смешивать peaks разных процессов с памятью одной VM.
4. Измерить стоимость invalidation/recompile и отсутствие неограниченного накопления code/metadata. Единичный короткий smoke не выдавать за leak-free stress.

**Готово:** capacity/lifetime сохраняются при длительной нагрузке, ресурсные ограничения и рост/разброс объяснены. Полный budget работающей гостевой ОС не заявляется.

#### S3-37e: native cached-memory fault/error lifecycle and republish gate

Added one exact local integration gate for the capacity-one
`MemoryPairCache` using its existing native published-memory backend. The test
precomputes uncached/interpreter references before retaining a native mapping,
then checks a cold publication and same-key hit with different invocation
CPU/RAM, STOP after the first committed store, and an architectural fault at
the second helper site. STOP and guest fault retain the resident entry. A
same-profile/same-generation dependency-version change then replaces the
occupied entry: `replacements` is 0 before and 1 after that transition, and
the replacement mapping is published and executed successfully. Explicit
clear, backend error, republish after error, and fresh-cache lifecycle do not
increment that counter. The test compares complete interpreter-visible
state, RAM, flags, RIP, retirement, callback order and memory accesses.

For second-site backend failure, the fixture captures the exact injected
`MemoryError::Backend` before normalization; both raw native paths report the
typed `MEMORY_ERROR_BACKEND` payload with address and length zero. The message
is fixture evidence and is not claimed to cross the native ABI. The backend
error removes the suspect entry; immediate successful publication and
execution verifies that the prior MAP_JIT mapping was released. The test then
clears/drops and opens a fresh cache with a different invocation state. This
bounded scenario is not leak-free stress, SMP reclamation, owner/MMU evidence,
or completion of S3-37.

The full local Stage 3 harness run
`stage3-20261101-154016-f87988c535` exited 0 with `local_only`: 106 steps
passed, 0 failed, 33 `not_run`. Its inventory has 102 Rust gate entries (92
acceptance, 10 lifecycle); the new S3-37e exact gate passed 1/1, and all 39
Python self-checks passed. Source fingerprints matched at start and end:
`767df064717d4a42d4788c289bf21eb148bee671d7287e96f9f428da269b3f76`. The run
records source revision `c87283ed2d9cdf9b71170b00864c5104cded6109`; commit
`d2932c1` contains the same tested implementation and harness files. VMHost
process, physical x86, Mac mini, performance, owner-backed and Stage 2 suites
were `not_run`. The run did not invoke or depend on Stage 2; Mac mini checks
remain deferred until the end of Stage 3. S3-37 and Stage 3 remain open.
Summary, manifest, logs and generated artifacts are preserved in
[`S3-37e local report data`](../reports/data/stage3/s3-37e-local/) and the
[bounded report](../reports/STAGE_3_S3-37e-local_2026-11-01.md).

#### S3-37f. Давление на кеш штатного S0-09 Session

Локальный gate `s3_37f_session_cache_pressure` выполняет 64 обхода 20
различных гостевых блоков в одном `od_host::session::Session`. Блок
записывает порядковый номер в RAX и завершается коротким переходом к
следующей границе. После каждого обхода отдельный RUN попадает в
последний резидентный блок. Проверяются полный CPU state, RIP, exit,
retirement, бюджет, 1 280 компиляций и публикаций, 1 264 вытеснения и
64 попадания. При 16 резидентных записях учёт code и logical metadata
остаётся стабильным между обходами, оба high-water значения укладываются
в лимиты.

STOP, выставленный перед RUN с заполненным кешем, оставляет CPU state и
счётчики прежними. `clear` освобождает публикации и lease MAP_JIT, обнуляет
текущий учёт и сохраняет накопленные счётчики. Тест подтверждает новый
lease, повторную компиляцию на очищенной сессии и исполнение в новой
сессии после уничтожения прежней. Это проверка локального ресурса
иммутабельного register-only Session. Измерения RSS и длительная нагрузка
S3-37b/d имеют отдельные сценарии. Production owner/MMU и SMC остаются
открытыми. Физические x86 и Mac mini проверки отложены.

Прогон `python3 tests/cpu/stage3_harness.py --suite local`
`stage3-20261119-040226-f23ee99ef0` завершился с `local_only`. Прошли
352 из 352 выбранных шагов, включая 334 Rust gate и 155 самопроверок
harness. Ещё 46 шагов получили `not_run`. S3-37f прошёл как один точный
тест `od-host --lib`. Начальный и конечный fingerprint дерева с
изменениями к `2b0f598` совпали
(`326302b49d983cfcaf43754510687f44d8678f45ef7ca3204604a5f8212f193c`).
SHA-256 summary
`d1ccb5b91ee33f15659bdeb6b1b9e26482c91123a50663a5c2eb6d4cd5411b30`,
SHA-256 input manifest
`f1d069345ca4326c9156a22b321839fe45474a98db11ff1c40aea96e9dd5ed89`.
Оба файла сохранены локально в `work/stage3-harness/` под ID прогона.

#### S3-37g. Стоимость инвалидации и первого запуска после неё

На FIFO16 измерены уведомление без удаления, удаление пары записей
одного backing и первые два запуска с новой версией. При начальной
заполненности 2 и 16 записей медианы удаления составили 0,221 и
0,364 мкс, а два запуска с перекомпиляцией и публикацией заняли около
5,25 мкс вместе. За 33 187 872 зачётных цикла освобождались два слота
на удаление, учёт кода и метаданных восстанавливался после двух
публикаций, вытеснений по ёмкости не было. Пара удалялась из начала
очереди между вызовами диспетчера. Измерения активной гостевой записи
и production owner/MMU остаются открытыми.
[Отчёт и сырые данные S3-37g](../reports/STAGE_3_S3-37g_2026-11-19.md)

#### S3-37h. Запись в активный V3-хвост и возобновление

В 4 202 880 циклах точный V3 store менял immediate ещё не исполненного
хвоста по прямому или алиасному адресу. Медианы вызова со store,
уведомлением и удалением двух записей составили 0,454 и 0,459 мкс.
Два первых вызова обновлённого хвоста занимали около 5,5 мкс вместе,
два попадания после публикации — около 1,38 мкс. В каждом цикле
проверены полное состояние, одна запись, отказ старого V3-handle,
освобождение слотов и восстановление текущего учёта после двух
публикаций. Отдельно измерена сборка активного V3 блока. Это данные
tracked fixture S3-24b. Общий active code и production owner/MMU
остаются открытыми.
[Отчёт и сырые данные S3-37h](../reports/STAGE_3_S3-37h_2026-11-19.md)

#### S3-37i. Длительное давление на штатный S0-09 Session

В каждом из двух подписанных процессов серия длилась не менее 660
секунд и включала 64 контрольные точки. На 20 гостевых блоках проверены полный
результат интерпретатора, попадания в хвост и точный учёт публикаций
и вытеснений. Всего проверено 335 331 840 публикаций.
При полном кеше каждый процесс удерживал 16 записей, 15 936 байт кода
и 38 144 байта логических метаданных. На каждой точке проверены STOP
перед RUN, отказ для недопущенного RIP, очистка, освобождение и повторный захват
lease MAP_JIT, заполнение и уничтожение сессии. RSS измерен
отдельно для каждого процесса до и после этих переходов.
[Отчёт, RSS и сырые данные](../reports/STAGE_3_S3-37i_2026-11-19.md)

### S3-38. Интегрировать runtime с VMHost

1. Подключить новый путь через существующий host ownership/lifecycle, сохранив старые публичные ABI guarantees. Новая версия ABI нужна только при реальной несовместимости и обновляется вместе со всеми consumers.
2. Проверить RUN/RESULT/STOP, повторные вызовы, destroy после завершения и корректное владение cache между requests.
3. Обеспечить независимость двух экземпляров: guest context, code cache и resources не смешиваются. Не использовать глобальный cache без требуемого isolation contract.
4. Повторить существующие control lifecycle/fault/process regressions и новый busy-JIT STOP. Host crash остаётся аварией helper, не гостевым исключением.

**Готово:** реальный VMHost исполняет принятый JIT slice и корректно завершает/очищает его. Это не означает готовность GUI или release distribution.

Повтор процессного набора 18 ноября 2026 года прошёл 40 из 40 выбранных
шагов, включая control race, изоляцию процессов и generated-fault smoke.
Исправлена отрицательная HELLO-фикстура [ISSUE-058](../KNOWN_ISSUES.md#issue-058).
Таймаут [ISSUE-044](../KNOWN_ISSUES.md#issue-044) в этом прогоне не встретился.
Команда и границы прогона приведены в [отчёте этапа](../reports/STAGE_3_REPORT.md#full-vmhost-process-selector-2026-11-18).

Повтор с S3-38j прошёл 46 из 46 шагов. Он включает отдельную подписанную
сборку для проверки закрытия соединения после `INVALID_STATE` и предыдущие
проверки control, process isolation и generated fault. Физические проверки
отложены. См. [отчёт S3-38j](../reports/STAGE_3_S3-38j_vmhost_v1_frames_2026-11-03.md#invalid_state-admission-batch-closure-2026-11-19-utc).

#### S3-38a. Register-only S0-09 integration through the existing V0 ABI

The current `od_fragment_execute` C ABI routes immutable, create-time validated
S0-09 fragments through `run_multi_cached_controlled` with the eager block
dispatcher. The full fragment is admitted before execution, and each synchronous
call owns a bounded arena/cache on its execution thread. The cache and published
mapping are dropped before the ABI call returns; no `!Send`/`!Sync` arena is
stored in the C handle or shared with Swift workers. V0 state/status layouts,
four GPRs, byte-offset RIP, raw RFLAGS and STOP lifecycle remain the external
contract. Invalid RIP remains `INVALID_STATE` before STOP/zero-budget dispatch.

At `RIP == code_end`, pre-run STOP returns `STOP_REQUESTED` and is cleared once,
following the documented V0 contract. The legacy fragment JIT returned
`CODE_END` first at this boundary; this slice records that precedence difference
and does not claim full legacy behavioral equivalence there. The immutable loop
fixture checks legacy/interpreter equivalence and resident-block hits; C ABI
tests cover repeat calls, independent handles, STOP, destroy, invalid-RIP status
precedence, and output preservation. Signed local VMHost control-race, process
isolation, and app-bundle regressions pass on the current macOS ARM64 host. The
generated-fault process smoke timed out once in three attempts and passed twice
on retry; its cause is unlocalized and is tracked in ISSUE-044.

This is partial S3-38 only. It does not establish persistent cache reuse across
requests, a whole VM session, general memory execution through the C ABI, full
owner/MMU integration, Mini support, or performance benefit. See the
[S3-38a report](../reports/STAGE_3_S3-38a_2026-10-29.md).

#### S3-38b. Reproducible local VMHost process suite

The Stage 3 harness has a separate `--suite vmhost-process` gate for the signed
local VMHost process scenarios. It builds `od-host` into a run-scoped Cargo
target, compiles launcher/VMHost/fault-helper into that run's artifacts folder,
records the resolved codesign tool and its hash, and captures per-step
commands, stdout/stderr, exit status, duration, and hashes. The control-race,
isolation, app-bundle, and generated-fault scenarios run exactly once per suite
invocation. Timeout evidence preserves captured diagnostics before process-group
cleanup; the harness does not retry a failed process gate. The preserved app
bundle is the copy actually executed and hashed. VMHost and fault helper retain
the required JIT entitlement; launcher entitlement evidence excludes it.

The 2026-10-30 process-only run passed all 33 gates on macOS ARM64. Its summary
and manifest are local ignored evidence under
`work/stage3-harness/stage3-20261030-022418-2f31426694/`. A subsequent
report-inclusive run passed the first 32 gates, then failed once at
`process_generated_fault` with `generated_fault_inner_timeout`; its launcher
stdout/stderr files are empty and process-group cleanup left no child. These
separate run records are preserved under `work/stage3-harness/`. An earlier
attempt is also preserved as failed because the harness lacked timeout entries
for its bundle gates; it stopped before generated-fault execution. The mapping
was fixed and covered by a one-to-one gate-map self-test. The passing observation
followed by a timeout leaves ISSUE-044 open and intermittent. `local_acceptance`,
Stage 1, memory corpus, physical x86, Mini, performance, owner-backed and Stage 2
remain separate and `not_run` in process-only evidence. See the
[S3-38b report](../reports/STAGE_3_S3-38b_2026-10-30.md).

**Current-source regression addendum, 1 November 2026:** a separate
`vmhost_process_only` run on local macOS ARM64 records 33 process steps passed,
0 failed, and 120 `not_run`; ten other suites, including Stage 2 and Mini,
remain unrun. Before it, one separate 40-check harness self-check had six
stale-fixture failures; after the fixture-only correction in `621e805`, the
self-check passed 40/40. Keep that self-check result distinct from the process
suite. Independent evidence review passed for the summary/manifest, selected
argv and logs, hashes, signatures/entitlements, process-group cleanup, and
artifact bindings. This does not resolve or close ISSUE-044 and does not
replace S3-38d RESULT-state coverage.
See the [current-source S3-38b addendum](../reports/STAGE_3_S3-38b-current-source_2026-11-01.md)
and its [committed evidence bundle](../reports/data/stage3/s3-38b-20261101/).

This bounded process suite does not establish persistent cache reuse across
requests, a whole VM session, general memory execution through the C ABI,
owner/MMU integration, Mini support, or performance benefit.

The V0 process isolation smoke now validates the entire survivor RESULT
after SIGSEGV in its peer. Its loop writes a marker to RCX and preserves
flags. RCX and RIP must match the reported instruction count, including
the valid zero-instruction STOP after RUN_ACK. Repetition on the same
signed macOS ARM64 helper reproduced the old assertion failure twice in
40 runs and passed all 100 runs with the correction. [ISSUE-059](../KNOWN_ISSUES.md#issue-059).

#### S3-38c. Sequential cross-thread V0 handle lifecycle

The focused `od-host` C ABI regression executes the same idle handle
sequentially on two distinct host threads, interleaves a second handle after
each synchronous call, and verifies state/RIP/retirement plus pre-run STOP
consumption. The successful second-handle calls also verify that the per-call
MAP_JIT mapping was released before return. It does not test simultaneous
execution, persistent cache reuse, or parallel MAP_JIT mappings. This is a
partial lifecycle slice; see the
[S3-38c report](../reports/STAGE_3_S3-38c_2026-10-30.md).

#### S3-38 runtime cache slice: eager composed NativeRegisterV1

Add a separately named opt-in `run_cached_native_register_v1_controlled`
entrypoint backed by the existing `SingleEntryBlockCache`. The caller owns the
immutable `CodeView`, complete dependency spans and frozen execution-context
generation. NativeRegisterV1 uses a distinct semantic profile and compile
flavor; the general S0-09 fixture admission and default runtime remain
unchanged. Compile eagerly on miss, retain one published handle on hit, and
preserve exact bare-NOP interpreter retirement and STOP/event guard order.

The local gates cover same-source/profile isolation, bytes/dependency/context
changes, one-entry replacement and handle release, failed publication or
execution, branch-loop reuse, NOP fallback, and interpreter/uncached-eager
full-state parity. The focused implementation and evidence are in the
[runtime cache report](../reports/STAGE_3_S3-38b_NATIVE_REGISTER_CACHE_2026-10-30.md).
This remains a fixture-level cache experiment: it does not provide SMC or
owner/MMU invalidation, persistent VMHost/session ownership, Mini support,
performance evidence, or Stage 2 integration.

#### S3-38c. Cached demand-flags NativeRegisterV1 candidate

Add a separately named opt-in candidate entrypoint over the same bounded
single-entry cache loop. Retain the NativeRegisterV1 semantic profile while
assigning a distinct compile flavor so candidate and eager artifacts cannot
cross-hit. Pending flag state remains execution-local. Exact gates cover
strategy-key isolation, candidate cache lifecycle/guards, and cold/hit full
state parity. This is experimental correctness/reuse evidence only; eager
remains the default and Mini/performance checks remain deferred.
Implementation evidence: [S3-38c report](../reports/STAGE_3_S3-38c_NATIVE_REGISTER_CACHE_CANDIDATE_2026-10-30.md).

#### S3-38d. Полный RESULT state через VMHost control transport

Локальный bounded-срез подтвердил полный 64-байтовый V0 RESULT для цикла
`48 B8 01 00 00 00 00 00 00 00 75 F4` при бюджетах 0–4, включая регистры,
RIP, RFLAGS, reason, reserved и retired; launcher JSON projection сверена
отдельно с его фиксированным начальным RFLAGS. STOP проверен с порядком
`RUN_ACK → STOP_ACK → RESULT`: immediate STOP не показывает прогресс гостя,
delayed STOP вернул 19 404 retired-инструкции. Это transport/lifecycle
correctness evidence, не измерение производительности и не полная проверка
VMHost process suite. Принят только финальный прогон, 14/14 шагов; пять
предварительных отказов и их исправления сохранены в отчёте для трассировки.
Полный набор VMHost process, physical x86, Mini, performance, owner-backed,
Stage 2 и ISSUE-044 не запускался; Mini остаётся отложенным до конца Stage 3.
S3-38 и Stage 3 остаются открытыми. См. [S3-38d report](../reports/STAGE_3_S3-38d-local_2026-11-01.md).

Повтор 18 ноября 2026 года восстановил проверку на текущем VMHost. Harness
компилирует `LinuxDirectRunner.swift` вместе с `VMHostMain.swift` и фиксирует
его digest в обоих VMHost manifests. Селектор `vmhost-result-state` прошёл
14 из 14 шагов. См. [результат повтора](../reports/STAGE_3_S3-38d-local_2026-11-01.md#vmhost-build-regression-2026-11-18).

#### S3-38e. Bounded local Rust session owner

The opt-in `od-host::session::Session` owns a copied S0-09 code fragment and
admission, frozen fixture identity/dependency inputs, a bounded multi-entry
cache, and one executable-memory lease. It borrows fresh CPU state, budget, and
STOP for each call; it is thread-affine, and `clear`/drop release the mapping
lease. The V0 ABI and its per-call lifecycle remain unchanged. This local Rust
slice does not route VMHost IPC RUN requests through a persistent session and
does not add `od-machine` or MMU/SMC integration.

Four exact `od-host --lib` lifecycle gates cover same-code publication reuse
with fresh state/control, cleanup, rejection of a second executable-memory
owner, and explicit invalid-entry errors without retry. S3-34dp pins them; all
four passed 1/1, and its fail-closed harness self-test passed. No performance
claim follows. See the
[S3-38e implementation report](../reports/STAGE_3_S3-38e_session_owner_2026-11-03.md)
and [S3-34dp harness report](../reports/STAGE_3_S3-34dp_2026-11-03.md).

#### S3-38f. Rust C ABI proxy for the persistent S0-09 session

Implement the additive `od_session_*_v1` ABI from the public header. One opaque
proxy owns one dedicated Rust OS thread; that thread alone constructs, executes
and drops the already thread-affine `Session`. Preserve the `od_fragment_*`
V0 ABI and lifecycle. Creation performs the first-entry preflight before the
owner acquires MAP_JIT. Each accepted RUN copies its request, consumes a
monotonic ID, validates immutable code and RIP, creates a fresh CLOEXEC pipe,
and gives the owner one request. STOP is invocation-local; the owner publishes
the terminal result before notifying, and wait consumes it once after signal
completion. Destroy is BUSY until the accepted invocation is consumed.

The code slice adds three exact Rust lifecycle identities and a fail-closed
Stage3 harness self-check. Each exact Rust gate passed 1/1, and the Python
self-check exited 0. The signed native C smoke checks public layout, linking,
normal notification and a deliberately lost notification reader. The owner
returns `INVALID_STATE` and the client can destroy the failed session and
acquire a fresh one. Descriptor exhaustion verifies `ALLOCATION_FAILED`
during pipe creation and recovery at a greater request ID. Both native
scenarios were repeated on M1 Pro. The signed local C smoke also sends
two waiters into the same active invocation. One receives `BUSY` before
STOP and the other consumes the single result with one notification.
The Mac mini, physical x86 and full
S3-38 acceptance remain open. See the
[S3-38f report](../reports/STAGE_3_S3-38f_session_proxy_2026-11-03.md).

#### S3-38g. VMHost control V1 repeated same-code RUN transport

The Swift VMHost now selects control protocol V1 only when the initial HELLO
frame explicitly carries version 1. It imports the public session ABI header,
creates at most one same-code S0-09 session, admits strictly increasing RUN
IDs, handles exact-ID STOP, and waits on the control socket and each fresh
completion descriptor from one control thread. Frame input is incremental and
limited to one maximum-size frame of staging; nonblocking output is bounded to
16,416 bytes. The V0 handshake, one-RUN lifecycle, and frame bytes remain on
the separate version-0 path.

Independent Sol/high static review passed after corrections for STOP status,
socket backpressure, `INVALID_STATE` admission closure, completion teardown,
and input staging bounds. No Swift build, test, signing, or runtime execution
was performed at the S3-38g integration revision. The later S3-38h process
smoke built and signed VMHost and passed same-code state/budget execution. S3-38g
remains partial pending STOP and completion races, monotonic/mismatched IDs,
malformed and partial frames, EOF while idle/running, backpressure behavior,
completion and descriptor/lease teardown, and unchanged V0 regressions on a
supported macOS ARM host. See the [S3-38g implementation report](../reports/STAGE_3_S3-38g_vmhost_v1_transport_2026-11-03.md).

#### S3-38h. Process-level repeated same-code RUN smoke

The process smoke drives one built VMHost over a local socket pair using
control V1. It sends `48 03 C3 48 03 C3` (two ADD RAX,RBX instructions) in two
increasing RUN IDs with distinct CPU snapshots and budgets. The smoke verifies
RAX `0x55`/RFLAGS `0x206` after the budget-one first run and RAX `0x1E9`/RFLAGS
`0x202` after the budget-two second run, full RESULT payloads, ACK-before-RESULT
ordering, and clean EOF teardown while idle. The process command and its source
dependencies are pinned in the Stage3 harness; the exact smoke and manifest
self-test passed. Release build, Swift compile, ad-hoc runtime signature, strict
signature verification and `allow-jit` entitlement verification passed. A
private typed bridge resolves Swift import of the canonical public ABI macros;
public ABI values and runtime behavior are unchanged. The full 39-step process
suite was attempted with `python3 tests/cpu/stage3_harness.py --suite
vmhost-process`, but the context-mode tool call timed out after 300 seconds with
no suite summary and an empty captured log; its outcome is unconfirmed (neither
a pass nor a test failure). STOP during RUN, EOF during RUN, malformed/partial
frames, broader V1 race/backpressure suite, and unchanged V0 regressions remain
open. The Graphify refresh timed out after 120 seconds and is unconfirmed. Mini
SSH timed out before remote tests; M1 remains deferred. See the
[S3-38h implementation report](../reports/STAGE_3_S3-38h_vmhost_v1_process_2026-11-03.md).

#### S3-38i. Process-level STOP isolation and active-run EOF

The smoke runs the admitted `75 FE` loop with ZF clear, sends exact-ID STOP
after RUN_ACK, and checks STOP_ACK before the terminal RESULT with STOP reason.
It then runs the same code with ZF set and a one-instruction budget to prove a
STOP request does not carry into the next invocation. In a separate process
case, the harness closes the control socket after RUN_ACK and VMHost must exit
cleanly within the bounded timeout. No positive retirement count is required
after STOP. Independent
Sol/high static review passed. The exact smoke command
`python3 tests/system/vmhost_control_v1_stop_eof_smoke.py /tmp/s3-38h-run/artifacts/od-vmhost-smoke`
passed with marker `VMHost control V1 smoke passed: STOP isolation and bounded
active-run EOF teardown`; the shared process-manifest self-test passed in
S3-38h. Process termination does not prove descriptor or executable-lease
cleanup before exit. Mini SSH timed out before remote tests; M1 remains deferred.
See the
[S3-38i report](../reports/STAGE_3_S3-38i_vmhost_v1_stop_eof_2026-11-03.md).

#### S3-38j. Process-level framing and admission checks

The smoke sends HELLO and a maximum-length RUN in header/body fragments,
exercises coalesced RUN frames, checks partial-header/body EOF teardown, and
submits an oversized declared payload without a body to pin early rejection.
The fragmented RUN carries 4096 code bytes. Its final byte is withheld before
admission and controls the RIP of the last `JNZ` executed with budget one.
The coalesced fixture uses two decoder-supported `ADD RAX,RBX` instructions
and expects the second RUN to receive scope-1 BUSY while the first invocation
finishes with its exact RESULT. A dedicated signed fixture injects scope-1
`INVALID_STATE` at create and begin. It verifies that a following STOP was
read with the RUN, receives no STOP ACK and checks EOF after the ERROR.

V1 sets `O_NONBLOCK` after READY so output pressure leaves the reactor free
to poll the completion descriptor. The process smoke pauses client reads
until the client cannot continue writing and ACKs are pending. It then drains
8192 exact, ordered STOP ACKs, completes another RUN and checks clean idle
EOF. A separate scenario holds output unread while sending the matching STOP
for an active RUN. The test observes retirement of its completion pipe before
draining and checks the ordered STOP ACKs and terminal RESULT. The staging
audit pins the decoder prefix at at most 8208 bytes and the response queue at
at most 16416 bytes. Signed diagnostic traces observed the 8208-byte decoder
boundary and exercised both idle and active output pressure.
See the [S3-38j report](../reports/STAGE_3_S3-38j_vmhost_v1_frames_2026-11-03.md).

#### S3-38k. Process-level immutable-code mismatch and request IDs

After code A succeeds as RUN ID 1, the smoke sends different code B as ID 2
and checks scope-1 `CODE_MISMATCH`. It retries ID 2 and checks
`INVALID_REQUEST_ID`, then sends B as fresh ID 3 and checks another
`CODE_MISMATCH` to prove the original binding remains. Reusing ID 3 is also
rejected; ID 4 with code A succeeds using fresh CPU state. The process harness
pins the exact command, 30-second timeout, source fingerprint, marker, and
VMHost binary hash. Independent Sol/high review passed. The focused smoke and
both manifest tests passed locally on ARM64. The later signed 46-step S3-38j
suite covered this gate and INVALID_STATE batch closure. Physical V1
acceptance is deferred. See the
[S3-38k report](../reports/STAGE_3_S3-38k_vmhost_v1_code_mismatch_2026-11-03.md).

#### S3-38l. Process-level active STOP and RUN-ID high-watermark

The smoke holds RUN ID 1 in an active loop, rejects STOP ID 2, duplicate RUN
ID 1, BUSY RUN ID 3, and decreasing RUN ID 2 with exact responses. Correct
STOP ID 1 must receive STOP_ACK before the terminal RESULT; RUN ID 3 then
remains stale because BUSY consumed it. RUN ID 4 proves the session remains
usable with fresh registers, flags, and budget. The process harness pins the
command, 30-second timeout, source fingerprint, success marker, and VMHost
binary hash. Two independent Sol/high static reviews passed. The focused smoke
and manifest test passed locally on ARM64. The later signed 46-step S3-38j
suite covered this gate and INVALID_STATE batch closure. Physical V1
acceptance is deferred. See the
[S3-38l report](../reports/STAGE_3_S3-38l_vmhost_v1_run_stop_id_2026-11-03.md).

#### S3-38m. Process-level RUN-ID exhaustion and fresh helper

The smoke accepts a one-instruction RUN at ID `0xffffffff` and verifies exact
ACK/RESULT frames. It then rejects a repeated maximum ID and ID 1 with exact
scope-1 `INVALID_REQUEST_ID`, demonstrating no wrap on the exhausted helper.
After clean idle EOF and helper exit, a newly launched helper/socket accepts
ID 1 with exact fresh state. The harness pins the command, 30-second timeout,
source fingerprint, marker, VMHost binary hash, and fail-closed self-check.
Independent Sol/high review passed. The smoke and manifest test passed locally
on ARM64 using the previously built VMHost; full process-suite acceptance is
unconfirmed after an earlier timeout, and physical Mini acceptance was not
run. The gate does not imply automatic reconnect or replay. See the
[S3-38m report](../reports/STAGE_3_S3-38m_vmhost_v1_id_exhaustion_2026-11-03.md).

#### S3-38n. V1 process isolation after a helper SIGSEGV

The separate V1 smoke launches two independent VMHost helper processes. Each
accepts a long-running admitted `75 FE` (`JNZ -2`) RUN with distinct GPR state.
The harness sends SIGSEGV only to helper A and requires signal termination;
helper B stays alive, accepts exact-ID STOP, and returns STOP_ACK before the
full 64-byte RESULT with its own unchanged state. Helper B then accepts the
same code under a new ID with ZF set and budget 1, returns a normal full result
with fresh registers and one retired instruction, and exits cleanly after idle
EOF. All socket and process waits are bounded, with kill/reap cleanup in a
`finally` path. The existing V0 process-isolation gate remains separate.

The exact process-suite step pins the smoke command, 30-second outer timeout,
source hash, success marker, and rebuilt VMHost binary hash. Its dedicated
fail-closed harness self-check verifies command/source/marker drift rejection.
The focused smoke and that exact self-check passed on the current local
macOS ARM64 source build; the full 40-step `--suite vmhost-process` was not run.
This proves local process isolation for this admitted-code scenario only. It
does not resolve ISSUE-044, validate generated-fault retry behavior, or close
the broader V1, Mini, or Stage 3 gates. See the
[S3-38n implementation report](../reports/STAGE_3_S3-38n_vmhost_v1_isolation_2026-11-03.md).

#### S3-38o. Production host loop одного vCPU для PagedFetchedLoadSession

Статус: принято с ограничениями после gate-ревью Claude Opus (high, 2026-11-18); дефект Д1 (S4-16i при `StopRequested` с retired 1) исправлен при приёмке, Д2 (нет детерминированного теста цикла на retired 1) оставлен ограничением. Ограничения: один vCPU и один поток; одна инструкция на блок, допуск на каждом native-шаге, производительность не измерялась; узкий интерпретатор платы (у форм только для JIT нет fallback и оракула); S3-27 открыт; S3-38 не закрыт. Оценка: 2–3 дня-агента. Зависимости: S3-25 (S3-25c), S3-28 (S3-28c), S4-15, S4-16 (приняты с ограничениями). Не зависит от S3-34 и от VMHost/ABI-части S3-38 (S3-38f–n). Закрытие S3-38o пункты 1–4 S3-38 не закрывает. Потребитель — S6-42: он подключает цикл к потокам vCPU и переводит допуск на слоты region (M1/M2 контракта S6-40); в этом срезе этого нет.

**Где:** `crates/od-host/src/` (рядом с `PagedFetchedLoadSession`, допустим новый модуль), тесты `od-host`. Не трогать: `crates/od-isa/src/` (линия S4-38; интерпретатор — только через существующий публичный API), `crates/od-host/src/storage_abi_v1.rs`, VMHost/Swift, C ABI V0/V1.

Замечание Д2 закрыто детерминированной регрессией цикла на ARM64. Тест поднимает STOP между записью RAM и проверкой байта в сгенерированном блоке. Проверены один retired store, выход на следующей безопасной точке и продолжение с нужной инструкции для обычной записи и записи в активную страницу кода. Результаты приведены в [отчёте S3-38o](../reports/STAGE_3_S3-38o_2026-11-18.md#deterministic-post-commit-stop-regression).

**Сделать:**
1. Цикл сессии одного vCPU. Каждая итерация начинается на safe point S3-25c: результат и счётчики проверены, уведомления `committed_write` применены, следующий lookup не начат. Затем один шаг: JIT-путь `PagedFetchedLoadSession` для допущенных форм. Если сессия отказала без эффектов инструкции, делается ровно один шаг общего paged-интерпретатора — того же, которым плата исполняет гостя (`run_paged_probe`, `FirmwareBoard::run_quantum`). MOV-only `step_paged_mov` для общей программы недостаточен. Все записи RAM обоих путей идут через `commit_write` (поколение растёт).
2. Внешний STOP. `AtomicU8` принадлежит вызывающему, а не сессии, и передаётся в `RuntimeContextV1::stop_requested` каждого native-вызова. Выход `StopRequested` одноинструкционного блока — штатный исход без повтора, а не `JitHostFailure(InvalidOutcome)`.
3. Протокол S6-40 (§5, S3, шаги 1–2) в однопоточной форме. Запрашивающий ставит флаг запроса (`SeqCst`: STOP, событие, finish), затем байт. На каждой итерации (шаг JIT или fallback-шаг интерпретатора) цикл снимает байт (`SeqCst`) и перечитывает флаги (`SeqCst`); между отказом JIT и шагом интерпретатора флаги не перечитываются, это укладывается в границу одной инструкции. При поднятом флаге шаг не выполняется, цикл возвращается на safe point с причиной. Байт снимает только цикл. Набор флагов расширяемый: S6-42 добавляет `stop_all` и `split_owner`, порядок не меняется.
4. Общий бюджет на JIT- и интерпретаторные шаги: `retired_jit + retired_interp + remaining == incoming`.
5. Событие проверяется на safe point. Выход «событие» происходит до следующего шага, флаг цикл не снимает. STOP приоритетнее события.
6. Аренда `MAP_JIT` не держится дольше, чем у текущей сессии. После выхода и drop сессии она снята (M6).
7. Обновить столбец `PagedFetchedLoadSession` матрицы S3-28c в JIT_CONTRACT по факту новых тестов (здесь не делать — при реализации).

**Проверить** (реальный native, macOS aarch64, на других платформах явный skip):
- программа из допущенных JIT-форм и не-JIT форм, с гостевым циклом: состояние, RAM и retired совпадают с чистым интерпретатором; доли шагов Jit и Interpreter записаны, обе > 0;
- STOP из другого потока во время бесконечного гостевого цикла: выход `StopRequested`, состояние равно интерпретатору на том же retired, retired не превышает прогресс на момент запроса больше чем на 2 (грубая оценка; точная граница — ни одной инструкции после итерации, снявшей байт — доказывается протоколом и детерминированными hook-тестами с запросом после перечитывания, а не этим тестом); resume после снятия флага совпадает с интерпретатором, без повтора инструкции и эффектов;
- флаг, поднятый до перечитывания, — шаг не выполняется (retired 0);
- запись в страницу кода (интерпретатором и JIT-store): следующий lookup видит новое поколение и исполняет новые байты;
- бюджеты 0..N на смешанной программе равны интерпретатору; выход «событие» — на safe point до следующего шага; STOP приоритетнее события;
- после выхода и drop `MapJitLease::acquire` успешен; новый цикл на новой сессии работает.

**Готово:** S6-42 может подключить цикл к потокам vCPU без изменений этапа 3. Ограничения: один vCPU, один поток; профиль JIT — одна инструкция на блок; отображение на допуск сохраняется (слоты — S6-42); S3-27 открыт.

**Не делать:** не менять VMHost/ABI; не править `od-isa/src`; не расширять набор JIT-форм; не вводить потоки и `Send`/`Sync` для ручек; не заявлять закрытие S3-38.

**Результат реализации (2026-11-18; принято с ограничениями на gate):** модуль `crates/od-host/src/paged_host_loop.rs` (`run_paged_host_loop`, `PagedHostLoopControl`, `PagedHostLoopReport`, `PagedHostLoopExit`); сессия получила `execute_forms_with_stop` и варианты `*_with_stop` внутренних native-входов (существующие входы вызывают их с локальным нулевым байтом, поведение не изменено); общий шаг интерпретатора — `od_machine::run_paged_probe` с бюджетом 1 (плата использует его же), новый API в `od-machine`/`od-isa` не добавлялся. Новые 13 тестов `paged_host_loop::tests::s3_38o_*` (реальный native, macOS aarch64; на других платформах один явный тест «только интерпретатор»); столбец `PagedFetchedLoadSession` матрицы S3-28c и раздел «S3-38o: the caller of `PagedFetchedLoadSession`» обновлены в [JIT_CONTRACT](../specs/JIT_CONTRACT.md). Ограничения: один vCPU, один поток; профиль JIT — одна инструкция на блок (каждая неподдержанная форма сбрасывает блок, в смешанном цикле допуск повторяется); отображение на допуск сохраняется (слоты — S6-42); набор инструкций общего интерпретатора платы узок (формы только для JIT — `48 89 03`, `8B 03`, со смещением и RIP-relative — без fallback и без оракула); вариант StopRequested с retired 1 (байт поднят внутри helper) циклом учитывается, но детерминированного теста нет; **ограничение, закрытое на gate (2026-11-18):** переписывание выхода в `CodeInvalidated` и пометка `admitted.retired = true` срабатывали только при `exit == Complete`; теперь и при `StopRequested` с retired 1 (`store_invalidates_active_code`). До исправления: с S3-38o `StopRequested` с retired 1 (байт поднят другим потоком во время helper) стал достижим, и для store в собственную страницу кода `CodeInvalidated` не выставлялся, блок не помечался retired (устаревшее переиспользование и тогда закрывала проверка поколения в ключе допуска). S3-27 открыт; S3-38 не закрыт. См. [отчёт](../reports/STAGE_3_S3-38o_2026-11-18.md).

### S3-39. Повторить ARM64 smoke на M1 Pro

1. Зафиксировать реально доступные OS/toolchain, способ сборки/передачи собственного artifact и выбранную ad-hoc/Hardened Runtime конфигурацию.
2. Выполнить bounded arithmetic/multi-block/helper/STOP/publication-lifecycle smoke вне debugger. Проверить отсутствие необъявленных M3-specific host instruction assumptions.
3. Сохранить конфигурацию и результат. Если M1 недоступен, указать непроверенную конфигурацию и не объявлять широкий host support.

**Готово, ограниченный smoke:** 31 октября 2026 года на MacBookPro18,3 / Apple M1 Pro / macOS 14.6.1 проверен portable ARM64 artifact с deployment target 14.0 и ad-hoc Hardened Runtime. Arithmetic, taken multi-block branch, accepted S3-14 helper profile, cooperative STOP, and same-handle C ABI publication lifecycle smoke прошли вне debugger. Это не широкая гарантия host support; M1 Pro не заменяет x86 oracle и не закрывает Developer ID/notarization вопрос ISSUE-004. Подробности и raw evidence: [S3-39 report](../reports/STAGE_3_S3-39_M1PRO_2026-10-31.md).

### S3-40. Зафиксировать решения по измерениям

1. Для allocator, flags, lookup/cache, linking и внедрённых IR improvements записать сравниваемые варианты, inputs, условия, samples, correctness и результат.
2. Сопоставить cold/warm/resource tradeoffs; выбрать самый простой вариант, удовлетворяющий измеренным требованиям.
3. При недостаточных данных назвать решение предварительным. Не переносить успешные данные одного short fixture на весь JIT или гостевую ОС.
4. Архитектурно значимые решения оформить ADR в существующем каталоге, остальные — в отчёте и BENCHMARKS. Сохранить нужные малые raw data и команды повторения.

**Готово:** окончательные решения критичных участков основаны на данных. Усложнения без подтверждённой пользы удалены либо не внедрены; ограничения производительности видимы.

#### S3-40a. Предварительная сводка измерений и решений

Сопоставлены S3-17 lookup, S3-18d1 dispatch/cache, S3-35a cold, S3-36a
resident warm и S3-37a churn evidence. Зафиксированы текущие baseline/candidate
границы для allocator, flags, cache, linking и IR transformations, а также
необходимые сравнения. Общего production winner или нового default нет:
prepared-key lookup не заменяет runtime A/B, cold 1,010-case и warm 14-case
workloads не дают общего break-even, high-water не доказывает RSS/lifecycle.
Архитектура и код не менялись; ADR не требуется для этого предварительного
реестра. S3-40 и итоговая приёмка остаются открытыми. См.
[S3-40a report](../reports/STAGE_3_S3-40a_2026-10-30.md).

#### S3-40b: branch/helper and RSS evidence addendum

Added the S3-36b branch/helper warm comparison and S3-37b process-wide RSS
snapshots to the provisional decision ledger. Neither chooses a production
default or supports a cross-workload speedup/break-even conclusion. S3-40 and
S3-41 remain open. See the
[S3-40b addendum](../reports/STAGE_3_S3-40b_2026-10-30.md).

#### S3-40c: single-block flags comparison addendum

Added three S3-29f release runs to the provisional ledger: candidate/eager
cross-run median paired ratios are 1.003–1.086 across seven fixed cases, with
larger emitted candidate code. This supports no default change and does not
settle full-runtime performance. Eager stays the reference/default; S3-29,
S3-40, and S3-41 remain open. See the
[S3-40c addendum](../reports/STAGE_3_S3-40c_2026-10-30.md).

#### S3-40d: cached composed NativeRegisterV1 flags harness

Added the opt-in `cached-native-flags-compare` harness for the frozen S3-34p
1,200-input corpus (`fnv1a64:172fb17b01c502df`). Each input combines two or
three generated native forms with a JZ/MOV/JNZ suffix, optional NOP/back-edge,
or the four-form index-zero anchor. Preflight checks the corpus envelope and
input digest, recomputes interpreter outcomes, and requires full-state, exit,
retirement and remaining-budget parity, including the frozen native/fallback
retirement split for both cached strategies. First dispatch and repeat dispatch
are separate phases under the same capacity-one cache policy; repeat dispatch
does not claim hit-only behavior. Exact per-input cache deltas are checked for
both phases. Reports separate total successfully published code bytes from
resident high-water sizes. A bench-only checked byte counter runs once per
successful publication under `phase-observation`, with no hit-path work; its
latency cost is not independently measured. Three scored M3 Max release runs
completed on the frozen corpus. Candidate/eager paired-ratio medians were
1.020–1.029 cold and 1.014–1.025 repeat, with paired spread crossing parity.
The candidate published 14.27% more code in cold mode and 13.07% more in
repeat mode. No winner inference or default change is supported; eager remains
the default. One high-load run2 attempt is diagnostic only; the accepted run2
retry is noisy bounded evidence, and run3 included low-load Stage 2 activity.
Stage 2 process launches alone do not stop a run; actual load and paired spread
are recorded. Mini checks remain deferred to the end of Stage 3. S3-40 and
S3-41 remain open. See the
[S3-40d result report](../reports/STAGE_3_S3-40d_2026-10-31.md) and
[`BENCHMARKS.md`](../testing/BENCHMARKS.md#s3-40d-cached-composed-nativeregisterv1-eagerdemand-flags-comparison).

#### S3-40e: bounded checked-memory cold, warm, and cached-call evidence

**Acceptance:** Add the completed S3-35c cold direct-entry, S3-36d warm
direct-entry, and S3-36e cached full-call checked-memory measurements to the
provisional decision ledger, preserving their distinct timed boundaries,
sequence strata, host/toolchain, source fingerprints, and fixture limits.
Do not derive cache cost or break-even by subtracting results from different
timed paths. Keep production policy and the broader S3-36/S3-40/S3-41 and
hardware gates open.

**Result:** S3-18e supplies the bounded capacity-one checked-memory cache
prerequisite (`b1a6ccb130844c40a396155a8f84bda656bfa8c3`), limited to the two
frozen `StoreThenLoad`/`LoadThenStore` fixture profiles. S3-35c measured cold
direct entry, including compile, publication, execution, and per-input drops;
its 60 paired raw-total native/interpreter ratios had median 71.4765 and range
66.7736–93.9423. S3-36d measured a resident warm direct typed entry without
dispatcher lookup or cache activity; across 60 pairs per sequence, medians and
ranges were 1.3718847445 (1.3116083380–1.4203506060) for
`store_then_load` and 1.3662312961 (1.2865399535–1.4421564464) for
`load_then_store`. S3-36e timed the complete cached `MemoryPairCache::run`
call, including validation, lookup/counters, and typed host execution; across
60 pairs per sequence, medians and ranges were 2.7222892353
(2.6348242133–2.8855539942) and 2.7650079504
(2.6390215880–2.9278959372), respectively. S3-35c reports one corpus-level
aggregate across its 60 pairs for both sequences; S3-36d and S3-36e keep the
two sequence strata separate.

These ratios compare different native boundaries with separately timed
interpreter calls. They cannot be subtracted from one another to estimate
cache overhead, cache-only cost, or break-even. All evidence is limited to two
frozen S3-34a memory sequences on Apple M3 Max / aarch64 / macOS 26.7.1 / rustc
1.99.0. Keep profile fingerprints distinct from release build source hashes
and revisions:

| Slice | Profile source identity | Scored release build identity |
|---|---|---|
| S3-35c | Revision `6f9da043073188b5164ee81b543add51314981a0`; profile source SHA-256 `5f4948f477c6e21232da25951219dab32c163a0a445fcade1b4b61c43d9e364a`. | All three runs: revision `6f9da043073188b5164ee81b543add51314981a0`; build source SHA-256 `1b1f655e70e989e96e92c4f17cee0724ccf9ef6a0714104a7107a7880e6d4d15`. [Manifest](../reports/data/stage3/s3-35c/suite-manifest.json). |
| S3-36d | Revision `dee1c860895c94883be7ce5650f50585569291f8`; profile source fingerprint `2ba1b077bc7a905adae120a98436a1fbf4d9d7de157cc2d02692db1d08ee0177`. | All three runs: revision `dee1c860895c94883be7ce5650f50585569291f8`; release build source SHA-256 `16771ca3b7d4f5e227a6c42a87c52b2788eb6987f8e1256488058a1568fbb5b0`. [Manifest](../reports/data/stage3/s3-36d/suite-manifest.json). |
| S3-36e | Common 15-file benchmark/runtime profile fingerprint `cdc04dc1ca8041c68a44b371f5f6f0aec7829f0466c005eb05deecec6cf6dd0f`. | Scored run 1: revision `16e68fb57427f8749447347787666971f49b44e3`, build source SHA-256 `008ccea1d32c22fc34fdf9c0f642a4b3f6a6190705578626ca89801df362db40`; run 2: `2bd172469014fb27a70ae5875513c6d389a15e38`, `e32bec28f899f19d228de31730daf33ac72d867e4d514883cbf91b16848271c4`; run 3: `d24865d29646eb5741fbc412fb52f4140ee81943`, `c9dd54440ba68f23e33b5c071fc96b6f028e5201f47414494617a5618a3bf0ce`. [Manifest](../reports/data/stage3/s3-36e/suite-manifest.json). |

S3-36e's release preflight build revision `cea6d65f764bd2d4a5d8b564cf743945aae596ab` and build source SHA-256 `a56d73670c3c273f555ff56fdb0d7a68ddcfae40695c4f04bc4ed9d7da4db63f` identify the preflight build only, not the three scored builds. The S3-36d profile fingerprint is not its release build SHA. Full identities and artifact manifests are preserved in the linked source reports and manifests.

The S3-35c suite-manifest SHA-256 is
`55b20439bf3b78366b803ada9744966cd2f995d4f52c4826fc19e907193344b1`; the
S3-36d and S3-36e manifest hashes are respectively
`ae855641e2643d7888558ffddf8ef29eb40021605e47ed8d27e8021058c5be19` and
`e643469ccb79ad69ebb9347f657c680f62b90c4a7e9c1d1c29111e4ad5a57c3f`.
S3-35c and S3-36d runtime dirty status is unknown; S3-36e release source tree
was clean while runtime `working_tree_dirty` is unknown.

This bounded ledger update does not support general memory-performance,
production default/winner, owner/MMU/SMC, whole-system, or broad cold/warm
claims. S3-36, S3-40, S3-41, and physical hardware gates remain open; Mini
checks remain deferred until the end of Stage 3. See the
[S3-40e evidence report](../reports/STAGE_3_S3-40e_2026-11-01.md) and the
[S3-35c](../reports/STAGE_3_S3-35c_2026-11-01.md),
[S3-36d](../reports/STAGE_3_S3-36d_2026-11-01.md), and
[S3-36e](../reports/STAGE_3_S3-36e_2026-11-01.md) source reports.

#### S3-40f: allocator comparison and correctness decision addendum

**Acceptance:** Synthesize S3-30a/b configuration-cost profiles separately
from S3-30p3/p4 bounded correctness evidence. Preserve fixture and timer
boundaries; distinguish the existing allocator configuration comparison from
an algorithm comparison. Record the current decision and next missing gate
without changing production policy or closing S3-30/S3-40.

**Result:** The register-homes budget4 baseline remains. Diagnostic budget0
is not an improvement or alternative allocator. S3-30a same-target and S3-30b
distinct-target measurements are not pooled; p3/p4 cover their named helper
and pending-flags fixtures only. Existing evidence is insufficient to change
allocator/default. The next allocator decision gate needs a representative hot
workload and a motivated alternative compared under the same conditions, with
correctness, compile cost, code size, and warm execution retained. Broader
workloads and production owner/MMU integration are outside these results; full
S3-30 and S3-40 remain open, as do full Stage 3 and Mini/physical gates. No new
architecture decision or ADR was made. See the
[S3-40f ledger addendum](../reports/STAGE_3_S3-40f_2026-11-01.md).

#### S3-40g: aliased ADD allocator diagnostic A/B addendum

Added the completed S3-32a exact aliased `ADD RAX,RAX` W64 diagnostic to the
provisional measurement ledger. Three sequential release processes produced
60 pooled pairs per set; all 360 timed sides met the 200 ms floor. Candidate /
baseline pooled-pair medians were 0.9833 for compile, 0.9879 for JZ taken, and
0.9975 for JZ fallthrough, but every observed range crosses 1. The candidate
also reduced the fixture's emitted code by 20 bytes and frame by 16 bytes, with
one fewer `ReadGpr`, temporary definition, and spill; helper counts were
unchanged. This is a bounded single-fixture allocator diagnostic: lower pooled
medians alone support no speedup, allocator winner, or default claim. The
broader S3-32 gate remains open; S3-40 and S3-41 remain open. Stage 2 activity is not a
blocker, and Mini checks remain deferred to the end of Stage 3. See the
[S3-40g addendum](../reports/STAGE_3_S3-40g_2026-11-01.md), the
[S3-32a source report](../reports/STAGE_3_S3-32a_2026-11-01.md), and its
[suite manifest](../reports/data/stage3/s3-32a/suite-manifest.json).

#### S3-40h: cached register recurrence performance addendum

Added the S3-36f full cached-dispatch versus interpreter measurement to the
provisional decision ledger. Three release processes retained 60 pairs per
stratum across six fixed synthetic recurrence strata; every ratio range was
above 1, so the interpreter was faster in all six. This confirms no useful
native target scenario in this synthetic fixture and supports no cache/runtime
policy change. It does not generalize to guest software or close S3-36, S3-40,
or S3-41. First-dispatch cost and code/logical cache sizes remain separate
measurements; no break-even inference is made. Mini remains deferred. See the
[S3-40h addendum](../reports/STAGE_3_S3-40h_2026-11-01.md) and
[S3-36f report](../reports/STAGE_3_S3-36f_2026-11-01.md).

#### S3-40i: cached dispatcher phase attribution addendum

Added the S3-31b ordinary/observed phase measurements to the provisional
ledger. The three measured phase shares are bounded to the S3-36f synthetic
recurrence kernels; they include observation perturbation and leave repeated
validation/control work in a residual. The paired observed/ordinary delta is
not isolated observer cost, and `b_64` crosses parity. These data do not
establish total lookup cost for representative guest workloads, a linking
benefit, or any runtime/cache policy change. S3-31, S3-36, S3-40, and S3-41
remain open; Mini remains deferred. See the
[S3-40i addendum](../reports/STAGE_3_S3-40i_2026-11-01.md) and
[S3-31b report](../reports/STAGE_3_S3-31b_2026-11-01.md).

#### S3-40j: profile-specific audit of saved S3-31c reports

Commit `5c313e758377e59c737961a9b237c71853e6a85e` adds a standard-library
validator scoped to the saved S3-31c process reports and optional preflight.
It checks profile/source compatibility, fixed strata and workloads, pair
identity and schedule, positive timings and the 200 ms floor, then recomputes
candidate/baseline and candidate/interpreter ratios and min/median/max ranges
from raw elapsed samples. The optional preflight validator checks its schema,
source/build identity, state/retirement/budget parity, and partial controls.

Evidence: `python3 -u tools/bench/test_validate_s3_31c.py` passed 16 tests;
independent Sol code review passed. The CLI audit of the three archived
process reports plus the clean untimed preflight returned `status: passed` for
all six strata and 60 pairs per stratum, with every retained side at or above
200 ms. The audit JSON and its hash are in the
[S3-31c evidence bundle](../reports/data/stage3/s3-31c-20261101/validator-audit.json).
This is a profile-specific audit of existing S3-31c data, not a second
benchmark, full S3-40 acceptance, or Stage 3 completion. Broader Stage 3 and
Mini checks remain open/deferred. See the [bounded report](../reports/STAGE_3_S3-31c_2026-11-01.md).

#### S3-40k. Повторные входы в S0-09 dispatcher

Для шести countdown inputs проверили SHA-256 файлов и пересчитали
сохранённые samples S3-35e/f и S3-36g/h встроенными Python-аудитами.
В экспериментах сверяли одинаковую работу гостя на Apple M3 Max
с macOS 26.7.1 и rustc 1.99.0. Оба столбца ниже относятся к версии
диспетчера `2b0f598`.

| Цикл | Cold JIT / interpreter, S3-35f | Warm JIT / interpreter, S3-36h |
| --- | ---: | ---: |
| Один RIP, 1 итерация | 17.1386 | 1.4255 |
| Один RIP, 3 итерации | 6.4181 | 0.6241 |
| Один RIP, 64 итерации | 0.3937 | 0.1132 |
| Один RIP, 4096 итераций | 0.0915 | 0.0867 |
| Два RIP, 64 итерации | 2.0451 | 1.7529 |
| Два RIP, 4096 итераций | 1.7905 | 1.7581 |

В таблице медианы по 32 cold и 24 warm парам на профиль. Каждый
диапазон лежит по одну сторону от 1. Cold включает создание `Session`
и первый RUN с публикацией. Warm измеряет RUN с резидентным кешем.
Порог окупаемости требует отдельного измерения.

Сохраняем оба изменения в штатном S0-09 dispatcher. Повторное использование
записи при возврате на тот же RIP в S3-36g снизило нормализованное время
до 7.60% и 5.77% прежнего результата на прогретых циклах в 64 и 4096
итераций. Короткий контроль и оба цикла с двумя RIP дали регрессию.
Переход на индекс, возвращаемый обработчиком cache hit, в S3-36h снизил
нормализованные медианы во всех шести профилях. В двух циклах с двумя RIP
медианы составили 97.18% и 96.98% прежнего JIT времени после нормализации.
Диапазон ненормализованного отношения времён для 64 итераций включил 1.
Интерпретатор быстрее в однопроходном контроле и обоих циклах с двумя RIP.
Вывод ограничен измеренными S0-09 профилями. Политика для более широких
нагрузок и оценка direct linking остаются открытыми.
[S3-35e](../reports/STAGE_3_S3-35e_2026-11-19.md),
[S3-35f](../reports/STAGE_3_S3-35f_2026-11-19.md),
[S3-36g](../reports/STAGE_3_S3-36g_2026-11-19.md),
[S3-36h](../reports/STAGE_3_S3-36h_2026-11-19.md)

S3-31g повторил два холодных и два прогретых двухблочных профиля с
повторным использованием предыдущей записи. В каждом профиле JIT
оказался быстрее интерпретатора во всех 32 парах. Отдельные одно- и
трёхблочные контроли ограничивают вывод этим чередованием RIP.
[S3-31g](../reports/STAGE_3_S3-31g_2026-11-19.md)

#### S3-40l. Сопоставление cold и warm одного пути памяти

На текущей сборке интерпретатор быстрее при первом вызове обоих
профилей. Прогретый кеш быстрее при сохранении той же записи.
Модель по парным замерам даёт 34–44 вызова до окупаемости, включая
первую публикацию. В эту оценку входят только измеренные интервалы
вызовов. Отдельный замер серии и причины разницы между сборками открыты.
[Решение по замерам S3-35h и S3-36k](../reports/STAGE_3_S3-35h_S3-36k_2026-11-19.md#reuse-estimate)

#### S3-40m. Локализация изменения времени интерпретатора

При общих счётчиках работы текущий `run` на двух профилях S3-34a
медленнее исторического примерно в 4,2 раза. Разница сохраняется
на двух прямых шагах и при отдельном декодировании. Изолированная
проба с известным набором выключенных профилей сокращает медиану
парного времени полного вызова примерно на 26%. Причина замедления
внутри декодера и оставшаяся часть времени исполнения открыты.
[Отчёт, замеры и исходники пробы](../reports/STAGE_3_S3-40m_2026-11-19.md)

#### S3-40n. Вклад общего пути декодирования двух MOV

На двух последовательностях S3-34a ограничение общего выбора операции
веткой MOV уменьшило медиану отдельного декодирования с 185–276 до
95–136 нс. Проба с тем же разбором префикса и операндов MOV, без общих
предварительных проверок, дала 36–47 нс. Совмещённая проба для полного
исполнения дала 212–316 нс с проверкой состояния, RAM и порядка обращений.
Точные затраты отдельных проверок и изменение производственного пути
остаются открытыми.
[Отчёт, замеры и исходники проб](../reports/STAGE_3_S3-40n_2026-11-19.md)

#### S3-40o. Сокращённый путь MOV с одним REX.W

Общий декодер использует тот же разбор операндов для первичных `89` и
`8B` с одним `48`–`4F`. Совпали полный результат и порядок fetch в
943 170 проверках. В парных локальных замерах медиана целевых MOV
составила 0.211 от исходной, контрольные группы дали 1.009–1.016.
Эффект для полного вызова требует отдельного подтверждения.
[Отчёт, замеры и исходники](../reports/STAGE_3_S3-40o_2026-11-19.md)

#### S3-40p. Полный вызов интерпретатора после изменения MOV

На 2 400 входах S3-34a новый декодер снизил медианное время полного
вызова на 33.7% и 33.4% на двух последовательностях. Все 64 пары
показали выигрыш, сравнение полного состояния, RAM, STOP и бюджета
прошло. Для исторического разрыва и отношения JIT к текущему
интерпретатору нужны отдельные прогоны.
[Отчёт, замеры и исходники](../reports/STAGE_3_S3-40p_2026-11-19.md)

#### S3-40q. Cold и warm JIT после изменения декодера MOV

На текущей сборке прогретый JIT занял 0.901 и 0.908 времени
интерпретатора по парным медианам двух потоков S3-34a. Холодный
вызов с компиляцией стоил 21.738 и 21.772 раза дороже. Исторический
интерпретатор всё ещё быстрее примерно в 2.8 раза. Результат
локален для этих потоков и кеша ёмкостью 1.
[Отчёт, замеры и исходники](../reports/STAGE_3_S3-40q_2026-11-19.md)

#### S3-40r. Стоимость отключённых профилей интерпретатора

Специализированная диагностическая копия дала 0.629 и 0.628 времени
полного вызова текущего интерпретатора по парным медианам S3-34a.
Все 64 пары показали выигрыш, проверки состояния, RAM, STOP и бюджета
прошли. Для решения по основному пути остаются проверки соседних
сценариев и размера кода.
[Отчёт, замеры и исходники](../reports/STAGE_3_S3-40r_2026-11-19.md)

#### S3-40s. Выбор представления пустых профилей

В парных пробах с общим исходным телом `run` и шага
`Default` дал отношения 1.063 и 1.060, ассоциированная
константа — 0.700 и 0.699. Константа выбрана для проверки
соседних сценариев и размера машинного кода.
[Отчёт, замеры и исходники](../reports/STAGE_3_S3-40s_2026-11-19.md)

#### S3-40t. Пустые профили в обычном интерпретаторе

Обычный `execute::run` подключён к общему циклу и шагу со статически
пустой конфигурацией. Полные результаты совпали на четырёх группах
по 2 400 входов. Выигрыш обычного вызова подтверждён, разброс явно
настроенного XORPD пересекает единицу. Секция машинного кода
стенда выросла на 150 528 байт, у подписанного VMHost уменьшилась
на 45 760 байт. Требуется обновить историческое и JIT сравнения.
[Отчёт, замеры и исходники](../reports/STAGE_3_S3-40t_2026-11-19.md)

#### S3-40u. Повторное сравнение JIT с текущим интерпретатором

На двух потоках S3-34a `e74f721` дал warm/interpreter 1.125 и
1.128 и cold/interpreter 28.160 и 28.019. Модель окупаемости не
дала порога в 47 из 48 троек. Остаточное отношение времени
интерпретатора к `d24865d` составило 2.226 и 2.250. По этим
потокам польза checked-memory JIT на текущей сборке пока не доказана.
[Отчёт, замеры и исходники](../reports/STAGE_3_S3-40u_2026-11-19.md)

#### S3-40v. Переиспользование точного ключа checked-memory

`MemoryPairCache::run` переиспользует ранее проверенный ключ при полном
совпадении входных данных. На двух потоках S3-34a медианы прогретого
candidate/baseline составили 0.714 и 0.713 по шести раундам. Парные
warm/interpreter у кандидата составили 0.799 и 0.802. Cold/interpreter
остались около 27. Результат ограничен кешем ёмкостью 1, двумя
проверенными профилями и локальным ARM64.
[Отчёт, замеры и воспроизведение](../reports/STAGE_3_S3-40v_2026-11-19.md)

#### S3-40w. Раздельные замеры RUN, STEP и декодирования

На двух потоках S3-34a `d9ecaa2` занял 2.073 и 2.107 времени
исторического RUN, 2.794 и 2.872 времени двух STEP, 0.740 и 0.730
времени двух decode. Все шесть раундов на каждом потоке сохранили
направление каждой разницы. Следующий замер проверит пустые профили
в обычном STEP. [Отчёт и сырые данные](../reports/STAGE_3_S3-40w_2026-11-19.md)

#### S3-40x. Пустые профили в обычном STEP

Обычный STEP использует тот же специализированный исполнитель,
что и обычный RUN. На двух потоках S3-34a медианы двух STEP составили
0.747 и 0.743 от baseline. Выигрыш повторился во всех шести раундах.
Диапазоны RUN и decode пересекли единицу. Паритет подтверждён
на MOV, исключении памяти, REP со STOP и настроенном XORPD.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40x_2026-11-19.md)

#### S3-40y. Проверка checked-memory JIT на актуальной ревизии

На Apple M1 Pro сравнили `d9ecaa2` и `5275d67` в шести парных раундах
на двух последовательностях S3-34a. Прогретый JIT на `5275d67` занял
0.8094 и 0.8205 времени парного интерпретатора. Отношения JIT между
ревизиями пересекли единицу по разбросу. Холодный путь на тех же входах
занял около 31 времени интерпретатора. [Отчёт и сырые данные](../reports/STAGE_3_S3-40y_2026-11-19.md)

#### S3-40z. Сравнение RUN с бюджетами 0, 1 и 2

На M1 Pro сравнили `d24865d` и `5275d67` на тех же двух
последовательностях S3-34a. При бюджете 0 медианы парных отношений
составили 1.2845 и 1.2847, при бюджете 1 — 2.0754 и 2.0557,
при бюджете 2 — 2.1433 и 2.1400. Исторический разрыв сохраняется,
а разница в абсолютном времени растёт с числом завершённых инструкций.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40z_2026-11-19.md)

#### S3-40aa. Проверка границы fetch и decode внутри RUN

На бюджете 2 сравнили исходный RUN, сквозной вызов декодера через
общую точку входа и возврат заранее декодированных инструкций. На M1 Pro
разрыв между `d24865d` и `5275d67` составил 162.2–163.0 нс на исходном RUN
и 176.5–176.8 нс в варианте без fetch/decode во время замера. На всех
2 400 входах совпали состояние CPU и RAM, порядок обращений к данным,
определённые флаги и выход по бюджету.
ISSUE-521 остаётся открытой. [Отчёт и сырые данные](../reports/STAGE_3_S3-40aa_2026-11-19.md)

#### S3-40ab. Цикл RUN и два вызова общего STEP

На тех же двух последовательностях и бюджете 2 проверили три варианта
без FETCH. Разрыв между ревизиями составил 178.0–178.4 нс на прежнем
predecoded RUN, около 176.7 нс в RUN с общим STEP и 170.3–170.5 нс
на прямой паре вызовов этого же STEP. Дизассемблирование подтвердило
общую точку входа, а проверки подтвердили два `Advanced`, состояние
CPU/RAM и порядок обращений с их контекстами. ISSUE-521 остаётся открытой.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40ab_2026-11-19.md)

#### S3-40ac. Входные проверки STEP

На M1 Pro проверили те же 2 400 входов с исходной парой STEP и двумя
режимами общего диагностического STEP. В текущей сборке обход выбранных
проверок изменил медианы менее чем на 1 нс. Разрыв с `d24865d` при обходе
составил 167.4 и 168.0 нс. Дизассемблирование подтвердило общий вызов
декодера, а проверки подтвердили точные состояния и обращения на
обоих префиксах. Причина остаточного разрыва остаётся открытой.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40ac_2026-11-19.md)

#### S3-40ad. Финальная диспетчеризация MOV

На M1 Pro сравнили исходную пару и два режима общего STEP на тех же
2 400 входах. Прямой вызов существующего `transfer` перед финальной
диспетчеризацией дал медианы 1.0080 и 1.0147 от времени обычной
диспетчеризации в текущей сборке. Разрыв с `d24865d` сохранился на
уровне 170.3 и 170.7 нс. Проверены оба префикса, порядок обращений,
CPU и RAM. Атрибуция оставшегося разрыва продолжается в ISSUE-521.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40ad_2026-11-19.md)

#### S3-40ae. Проверки вариантов x87 на пути RUN

По профилю M1 Pro заменили шесть сравнений с вариантами x87 без
полей на `matches!`. Дизассемблирование измеряемого STEP подтвердило
исчезновение всех шести вызовов `Operation::eq`. При бюджетах 1 и 2
время обычного RUN сократилось на 5,6–6,7% по парным медианам.
Выигрыш повторился во всех шести раундах для обеих последовательностей.
При нулевом бюджете парные диапазоны включили единицу. Проверены 2 400
MOV-входов и 2 609 тестов `od-isa` для обеих сборок. ISSUE-521 открыта
для оставшегося исторического разрыва.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40ae_2026-11-19.md)

#### S3-40af. Сгруппированные проверки сравнения x87

Одна проверка `matches!` объединяет четыре смежных условия с тем
же помощником. Связанный STEP на M1 Pro содержит 90 вызовов
`Operation::eq` вместо 93. На двух MOV-последовательностях
при бюджетах 1 и 2 обычный RUN занял на 2,58–3,28% меньше времени
по парным медианам. Все шесть раундов показали выигрыш. Прошли
проверки 2 400 MOV-входов и 2 609 тестов `od-isa` для обеих сборок.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40af_2026-11-19.md)

#### S3-40ag. Остаточный разрыв обычного RUN

На M1 Pro сравнили `d24865d` и `5bc5ec6` на тех же 2 400 входах
с бюджетами 0, 1 и 2. При двух завершённых инструкциях парные
отношения медиан составили 1.9716 и 1.9770, а разница общих
медиан составила 134.2 и 134.7 нс. При нулевом бюджете разница
составила 5.3 нс. Все шесть раундов дали одно направление.
ISSUE-521 остаётся открытой.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40ag_2026-11-19.md)

#### S3-40ah. Проверки системных инструкций на пути RUN
По профилю M1 Pro четыре проверки RDTSC, CPUID, SYSCALL и SYSRET
заменены отдельными `matches!`. В связанном STEP исчезли все четыре
целевых вызова `Operation::eq`. При бюджетах 1 и 2 время обычного
RUN на обеих MOV-последовательностях сократилось на 5,91–6,70% по
парным медианам. Все шесть раундов дали выигрыш при ненулевом
бюджете. Диапазоны при бюджете 0 включили единицу. Обе сборки
прошли 2 400 MOV-входов и 6 007 тестов `od-isa`. ISSUE-521 открыта.
[Отчёт, профили и сырые данные](../reports/STAGE_3_S3-40ah_2026-11-19.md)

#### S3-40ai. Остаточный разрыв RUN после S3-40ah

На M1 Pro сравнили `d24865d` и `de1f446` по тому же расписанию.
При бюджете 2 парные отношения медиан составили 1,8436 и 1,8499,
а разница общих медиан составила 117,6 и 118,0 нс. При бюджете 0
она составила 4,7 нс. Обе сборки прошли проверку состояния и
обращений к памяти на 2 400 MOV-входах. ISSUE-521 открыта.
[Отчёт и сырые данные](../reports/STAGE_3_S3-40ai_2026-11-19.md)

#### S3-40aj. Проверки HLT и прерываний на пути RUN

Свежий профиль `de1f446` связал 42 стека с проверками HLT, INT3 и CLI.
Для соседней проверки STI подтверждён вызов в машинном коде.
Четыре внешних предиката заменены на `matches!`. В связанном STEP
осталось 81 сравнение `Operation::eq` из 86. На двух MOV-профилях
при бюджетах 1 и 2 парные медианы RUN снизились на 3,39–3,86%.
При нулевом бюджете диапазоны включили единицу. Обе сборки прошли
семь профильных тестов и 2 400 MOV-входов на каждом бюджете.
ISSUE-521 остаётся открытой.
[Профили, замеры и проверки](../reports/STAGE_3_S3-40aj_2026-11-19.md)

#### S3-40ak. Остаточный разрыв RUN после S3-40aj

Повтор на M1 Pro сравнил `d24865d` и `d48eef9` на тех же двух
MOV-последовательностях. При бюджете 2 разница медиан составляет
109,8 нс для обоих сценариев, парные отношения составляют 1,7952
и 1,7957. При бюджете 0 разница составляет 5,3 нс. Обе сборки
прошли проверку состояния и памяти на 2 400 MOV-входах.
ISSUE-521 остаётся открытой.
[Парный замер и архив](../reports/STAGE_3_S3-40ak_2026-11-19.md)

### S3-41. Оформить приёмку этапа

1. Создать STAGE_3_REPORT: exact profiles/forms, native/helper/fallback границы, revision/fingerprint, машины/toolchains, correctness counts, commands/seeds, cold/warm/resource результаты.
2. Обновить этот план, JIT_CONTRACT, JIT.md, BENCHMARKS и README затронутых компонентов по фактическому состоянию. CPU contract менять только при действительно изменившемся архитектурном поведении.
3. Зарегистрировать нерешённые дефекты в KNOWN_ISSUES: компонент, последствия, воспроизведение, статус и следующий шаг. Решённые закрыть regression evidence. Планируемые возможности не выдавать за обнаруженные баги.
4. Сверить матрицу ниже. Незакрытый prerequisite этапа 2/4 или hardware criterion не превращать в выполненную задачу изменением формулировки.
5. Если готов только срез A/B, назвать именно его. Первый JIT не обязан нативно переводить всю ISA, но обязательные runtime/correctness/performance условия выбранного этапного профиля должны быть выполнены.

**Готово:** одинаковые программы дают одинаковое определённое состояние interpreter/JIT/physical x86; cold и warm исполнение измерены отдельно, подтверждена польза на оговорённых целевых сценариях и честно описаны регрессии/ограничения.

#### S3-41a. Сводка локальных bounded acceptance evidence

Обновлён [локальный сводный отчёт](../reports/STAGE_3_REPORT.md) с матрицей,
разделяющей четыре статуса: bounded local evidence подтверждено в его
оговорённых пределах; широкие Stage 3 correctness/performance/workload
условия остаются открытыми; physical x86/Mini gate отложен до конца этапа;
production RAM/backing/mapping/MMU owner остаётся вне текущего fixture scope.
Локальные evidence включают S3-18e, S3-35c, S3-36d/e, S3-37d, S3-38 и S3-39,
каждое со своей ссылкой и ограничениями. S3-36e native/interpreter ratios
выше 1 на двух проверенных fixtures, поэтому полезный целевой сценарий пока
не доказан; различающиеся cold/warm/full-call таймеры не задают cache cost или
break-even. Сводный harness run 30 октября привязан к revision
`fa80ab0da77b3de268c6adf649dc2a92de9dde20` и fingerprint
`caa5eec7590595b66910ce855e6c6eba4b476b59c1f7704619b273eb10e970c9`; он не
покрывает более поздний HEAD, чьи slices опираются на собственные run evidence.
Stage 2 runs/activity сами по себе не являются blocker и не требуют quiet
окна. S3-41 и итоговая приёмка остаются открытыми; Mini checks отложены до
конца этапа. ISSUE-044 остаётся открытой; новую попытку VMHost process suite
без нового основания не запускаем. Изменение документационное, runtime не
менялся.

**Обновление, 1 ноября 2026 года:** принятый summary текущего full-local run
`stage3-20261101-190932-58293bb642` фиксирует 120 passed, 0 failed и 33
`not_run`; source revision `020f79d74566ad6abdc8d2fa4ad8d6e0bfaf65ea`,
fingerprint начала и конца
`8dfe432b2903d5b62c7de353665b0efd51597280a6cf86ca7223f7e18e8175f9`. Stage 1
child проверил локальное согласование interpreter/JIT-dispatch для 10 manual и
1 000 generated inputs, два повтора (2 020 результатов); `stage1_dispatch`
остался `local_only`. Другие suites — VMHost process, physical x86, Mini,
performance, owner-backed и Stage 2 — не запускались. Предшествующий запуск
`stage3-20261101-182017-d88d51f20b` остановился после 106 passed из-за
`FileExistsError` при создании уже существовавшего run-scoped `artifacts`
каталога; commit `020f79d` безопасно переиспользует настоящий каталог и
отклоняет symlink. Неудачный summary и manifest сохранены рядом с принятым
evidence. Независимая проверка evidence/bindings прошла без замечаний; S3-41
и Stage 3 открыты.
См. [датированный S3-41a report](../reports/STAGE_3_S3-41a-current-local_2026-11-01.md)
и [evidence bundle](../reports/data/stage3/s3-41a-local-20261101/).

**Обновление, 3 ноября 2026 года:** новая попытка `local` selector,
`stage3-20261103-123052-8fa49da8df`, остановилась до начала suite: обязательный
`harness_self_tests` превысил timeout 120 секунд, а завершения команды через
300-секундный context RPC получить не удалось. Первая остановка оставила
временный root-каталог, изменив fingerprint; каталог созданным тестом удалён,
начальный fingerprint восстановлен. ISSUE-050 фиксирует причину и follow-up:
timeout 600 секунд, временные fixture-каталоги в ignored `work/`, сохранение
первичной причины ошибки. Focused regression passed 1/1; post-fix full local
selector attempt exceeded the 240-second context RPC window without a final
summary, so its result remains unconfirmed and S3-41 stays open.

**Обновление, 18 ноября 2026 года.** Local selector на `3ece775`
(`stage3-20261118-221234-a03bfcd2f6`) остановился на семи устаревших
проверках счётчиков [ISSUE-060](../KNOWN_ISSUES.md#issue-060). В self-tests
сведены в одно место ожидаемые 152 self-tests, 326 Rust gates и 344 local
steps. Проверки точных имён, команд и дубликатов сохранены. Отдельно прошли
7 регрессий, а повторный selector `stage3-20261118-221625-51e161ab59`
прошёл `harness_self_tests` (152 из 152). Этот результат закрывает
ISSUE-060. Для S3-41 требуется итог полного прогона.

После восстановления сборки S3-38d локальный набор
`stage3-20261118-223445-2770f791c4` завершился в статусе `local_only`.
Все 344 выбранных шага прошли, 40 шагов остались `not_run`. Зафиксированы
локальный dispatcher corpus, memory corpus и VMHost RESULT. Команда,
fingerprints и границы результата приведены в [отчёте этапа](../reports/STAGE_3_REPORT.md#full-local-selector-2026-11-18).
Итоговая приёмка S3-41 остаётся открытой.

**Обновление, 19 ноября 2026 года.** Полный локальный selector после
S3-31g на `d99ce3b` завершился как `local_only`. Прошли все 359
выбранных шагов. Отдельный подписанный VMHost selector прошёл 46 шагов
на снимке исходников S3-31g. Физические x86 и Mac mini gates отложены.
Локальные результаты и открытые ограничения учтены в S3-41.
[Прогоны и привязка к исходникам](../reports/STAGE_3_REPORT.md#full-local-selector-after-s3-31g-2026-11-19-utc)

**Обновление, 19 ноября 2026 года.** После S3-40o на `e31c105`
прошли все 359 локальных шагов и 46 шагов отдельного локального
набора VMHost. Снимок исходников совпал в начале и конце обоих
прогонов. Исторические пять x87-тестов из ISSUE-040 проверены
отдельно в полном наборе `od-isa`. Физические проверки отложены.
[Результаты и привязки](../reports/STAGE_3_REPORT.md#local-and-vmhost-selectors-after-the-mov-decoder-change-2026-11-19-utc)

**Обновление после S3-40t, 19 ноября 2026 года.** Отдельный
подписанный локальный набор VMHost прошёл все 46 из 46 шагов на
`9a6e4e9`. Физические проверки отложены.
[Прогон и привязка к исходникам](../reports/STAGE_3_REPORT.md#signed-vmhost-selector-after-interpreter-specialization-2026-11-19-utc)

**Обновление после S3-40v, 19 ноября 2026 года.** На одном снимке
`d9ecaa2` прошли 359 из 359 шагов локального набора и 46 из 46 шагов
подписанного VMHost process suite. Обе сводки связаны с одним
отпечатком исходников и входным манифестом. Журналы и артефакты
дочернего прогона Stage 1 сохранены для повторной проверки. Физические
проверки отложены, итоговая приёмка S3-41 остаётся открытой.
[Результаты и архивы](../reports/STAGE_3_REPORT.md#local-and-signed-vmhost-selectors-after-s3-40v)

**Обновление после S3-40x, 19 ноября 2026 года.** На `5275d67`
прошли 359 из 359 локальных шагов и 46 из 46 шагов подписанного
VMHost process suite. Оба прогона связаны одним отпечатком исходников
и входным манифестом. Проверены хеши всех журналов и доказательства
дочернего прогона Stage 1. Итоговая приёмка и отложенные физические
проверки остаются открытыми.
[Результаты и архивы](../reports/STAGE_3_REPORT.md#local-and-signed-vmhost-selectors-after-s3-40x)

**Повтор на M1 Pro, 19 ноября 2026 года.** После исправления сборки
под Swift 5.10 на чистом `f6b5214` прошли 359 из 359 локальных шагов и
46 из 46 процессных шагов VMHost. Отпечаток исходников и манифест
входов совпали, все 405 выбранных шагов сверены с сырыми журналами.
Открыты критерии Mac mini и нативного сравнения на физическом x86.
Итоговая приёмка S3-41 остаётся открытой.
[Проверки и архивы](../reports/STAGE_3_REPORT.md#full-local-and-vmhost-process-checks-on-m1-pro-after-the-swift-fix)

**Повтор после S3-40ae, 19 ноября 2026 года.** На чистом `8c159c8`
прошли 359 из 359 локальных шагов на M1 Pro. VMHost прошёл 45 из
46 шагов. На `process_generated_fault` launcher сообщил закрытие
канала во время чтения кадра. Двенадцать повторов с трассировкой
на тех же подписанных артефактах получили статус SIGSEGV.
Исходный сбой и диагностические повторы сохранены в ISSUE-523.
[Проверки и архивы](../reports/STAGE_3_REPORT.md#m1-pro-checks-at-8c159c8)

**Исправление launcher.** Локальный тест воспроизвёл закрытие канала
до появления статуса SIGSEGV. Launcher теперь ограниченно ожидает
статус при оборванном кадре и сохраняет обычную очистку для живого
потомка. Пять случаев regression smoke и подписанный generated-fault
smoke прошли. [Проверки исправления](../reports/STAGE_3_REPORT.md#launcher-exit-reporting-after-a-truncated-frame)

**Повтор на M1 Pro после исправления launcher и S3-40af.** На чистом
`5bc5ec6` прошли 359 из 359 локальных шагов и 46 из 46 шагов VMHost.
Отдельные пять сценариев закрытия канала прошли на том же подписанном
launcher. `process_generated_fault` зарегистрировал SIGSEGV и
подтвердил очистку группы. Сводки и журналы проверены, отпечаток исходников
и входной манифест совпали. Физические критерии и итоговая приёмка
S3-41 остаются открытыми.
[Результаты и архивы](../reports/STAGE_3_REPORT.md#m1-pro-checks-after-the-launcher-fix)

**Повтор на M1 Pro после S3-40ah.** На чистом `de1f446` прошли все
359 локальных шагов и 46 шагов подписанного набора VMHost. Отпечаток
исходников и манифест входов совпали. Проверены журналы, включая SIGSEGV
и очистку группы в `process_generated_fault`. Отдельный подписанный
C-клиент проверил layout, обработку потери читателя и восстановление
после `EMFILE`. Физические критерии и итоговая приёмка S3-41 остаются
открытыми.
[Результаты и архивы](../reports/STAGE_3_REPORT.md#m1-pro-checks-after-s3-40ah)

**Повтор на M1 Pro после S3-40aj.** На чистом `d48eef9` прошли 359 из
359 локальных шагов и 46 из 46 шагов подписанного VMHost. После обрыва
SSH исходный локальный процесс завершил прогон. Отдельный подписанный
C-клиент использовал release-библиотеку из прогона VMHost и проверил
двух одновременных waiters, отправку уведомления и следующий RUN.
Этот прогон предшествует добавлению S3-34ds в локальный selector.
[Результаты и архивы](../reports/STAGE_3_REPORT.md#m1-pro-checks-after-s3-40aj)

**Расширенный набор на M1 Pro.** На чистом `2e9b601` прошли все 360
локальных шагов и 46 шагов подписанного VMHost. Локальный набор включает
S3-34ds и обновлённую проверку редьюсера S3-34dt. У обоих прогонов совпали
отпечаток исходников и манифест входов. Физические проверки остаются
отложенными.
[Результаты и архивы](../reports/STAGE_3_REPORT.md#expanded-local-and-vmhost-selectors-on-m1-pro)

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

#### S3-20cg. Address64 qword store из RSI/RDI в low-base direct memory

Добавить только singleton `48 89 /r` длины 3: ModRM `mod=00`, source RSI/RDI (`reg=6|7`), base RAX/RCX/RDX/RBX (`r/m=0..3`), без displacement и SIB. Admission остается operation-specific для `CheckedStore`; generic GPR whitelist, loads и frame/staging не расширяются. Store читает полное 64-битное значение source из canonical guest state. RSI/RDI как адресная база, addr32, displacement/SIB, REX.R/B, R8+ и другие неподдержанные формы отклоняются. См. [S3-20cg report](../reports/STAGE_3_S3-20cg_2026-10-28.md). Mini и performance отложены до конца Stage 3; owner/MMU и Stage 2 вне scope.

#### S3-20ch. Address64 qword load в RSI/RDI из low-base direct memory

Добавить только singleton `48 8B /r` длины 3: ModRM `mod=00`, destination RSI/RDI (`reg=6|7`), base RAX/RCX/RDX/RBX (`r/m=0..3`), без displacement и SIB. Admission разрешает `WriteGpr` для RSI/RDI только непосредственно после `CheckedLoad` с тем же temporary, qword operand, exact low-base direct адресом и длиной 3; `ReadGpr` и общий `validate_gpr` остаются прежними. Согласовать staged GPR frame: восемь qword слотов `[56,120)`, flags `120`, RIP `128`, сохранения X19/X20 `144`, X21/X22 `160`, reserved prefix `176` с 16-byte alignment. Shared emitter setup/commit, allocator/compiler prefixes и spills используют тот же layout; лимит кода остается 4096 bytes. Другие prefixes, addr32, displacement/SIB, extra REX, R8+ и ширины кроме qword отклоняются. Приёмка — direct/shared IR/interpreter full-state parity, high-bit transfer, single qword read/retirement, fault и budget guards без изменения state/access, frame regressions и прежние arithmetic/store/two-memory regressions. См. [S3-20ch report](../reports/STAGE_3_S3-20ch_2026-10-28.md). Mini/performance и owner/MMU/Stage 2 остаются вне scope.

#### S3-20ci. Address64 direct low-base qword MOV с RSI/RDI data и displacement

Добавить ровно 32 формы `48 8B/89 /r disp8|disp32`: ModRM `mod=01/10`, data RSI/RDI (`reg=6|7`), base RAX/RCX/RDX/RBX (`r/m=0..3`), qword EA/data, длины 4/7. Не добавлять SIB, `67`, дополнительные префиксы, другие base/data registers, pair или V3 формы. Effective address остаётся decoder-owned `MemoryOperand::access_address_48`: полный старый base плюс signed displacement с wrapping u64. Scoped IR разрешает RSI/RDI source только в соответствующем `CheckedStore` и `WriteGpr` только непосредственно после соответствующего `CheckedLoad`; общий `ReadGpr` и `validate_gpr` не меняются. Приёмка: direct/shared IR/interpreter parity для полных CPU/RAM, callback address/width/count и retirement; signed disp8/disp32 boundaries, wrap, noncanonical start/end #GP без callback, endpoint overflow, architectural/backend atomicity, store+STOP одним qword callback, budget guard, actual RIP continuation 4/7 без replay, source identity последнего байта disp32, partial windows 1..3/1..6 и отвергнутые соседние формы. Frame остается 176 bytes, code cap — 4096; сохраняется 10×MOV regression size 3868. См. [S3-20ci report](../reports/STAGE_3_S3-20ci_2026-10-28.md). Mini/performance и owner/MMU/Stage 2 отложены до конца Stage 3/вне scope.

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

The linked-worktree metadata fix in `f1670fc` passed independent static
Sol High review. On 2026-11-18, `python3 tools/check_layout.py` passed in the
`codex` linked worktree at `3476780` after restoration of the existing local
evidence. This verifies the layout and link check for that checkout.
Repeat the command at S3-41 acceptance. See the [layout evidence](../reports/STAGE_3_REPORT.md#layout-acceptance).

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

#### S3-20ca. Address64 low-base SIB без индекса

Добавить только singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB строго `0x20..=0x23` (scale 1, index field `100` как sentinel отсутствующего индекса, base RAX..RBX), data RAX..RBP, Qword EA/data; длины 4/5/8. EA остаётся decoder-owned: старый base плюс signed displacement. Не принимать RSP как индекс, другие SIB/базы, address32, новые data registers, пары или V3.

Приёмка: JIT descriptor и host direct V2/shared IR/interpreter parity для load/store по всем трём длинам, четырём базам и шести data registers, с signed displacement edges; сравнить с эквивалентным direct-base encoding. Проверить полное CPU/RAM, независимую EA, callback address/width/count и retirement; fault/overflow no-commit, store+STOP, entry guards, last disp32 byte identity, partial windows и invalid neighbors; actual-RIP dispatcher continuation через 4/5/8-byte forms без повторных эффектов. См. [S3-20ca report](../reports/STAGE_3_S3-20ca_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20cb. Address32 low-base SIB без индекса

Добавить только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB строго `0x20..=0x23` (scale 1, index sentinel `100` означает отсутствующий индекс, base RAX..RBX), data RAX..RBP, Dword EA/Qword transfer; длины 5/6/9. Эффективный адрес вычисляет decoder-owned `MemoryOperand::access_address_48`: `(low32(old base)+signed displacement) mod 2^32`, затем zero-extend. Base register сохраняется, если он не является destination load; при data=base load старое значение используется для EA, после успешного чтения полный qword записывается в destination. Не добавлять другие SIB, base/index, prefix, data register, pair или V3 forms.

Приёмка: JIT descriptor и host direct V2/shared IR/interpreter parity обеих операций по четырём базам, шести data registers и no-disp/disp8/disp32 signed boundaries; полное CPU/RAM, независимый EA oracle, callback address/width/count и retirement. Сравнить эффективное поведение с эквивалентным address32 direct-base encoding, корректируя только RIP для различной длины. Проверить high-half truncation, qword access через 4 GiB, fault/backend atomicity, store+STOP payload/retirement, entry guards, partial windows, ninth-byte target identity, invalid neighbors и actual-RIP dispatcher continuation через 5/6/9-byte forms без повторного эффекта. См. [S3-20cb report](../reports/STAGE_3_S3-20cb_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20cc. Address64 RBP-base SIB без индекса с displacement

Добавить только singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB ровно `0x25` (scale 1, отсутствующий index, RBP base), data RAX..RBP, qword EA/data; длины 5/8. Тот же SIB byte при `mod=00` остаётся отдельной absolute/no-base S3-20au формой и не является этим профилем. Effective address — только decoder-owned `MemoryOperand::access_address_48`: старый RBP плюс signed displacement с wrapping u64; RSP не участвует как индекс. Не добавлять address32, другие SIB значения, prefixes, data registers, пары или V3.

Приёмка: direct V2/shared IR/interpreter parity для load/store, шести data GPR и signed disp8/disp32 границ; независимый EA oracle, полный CPU/RAM, callback address/width/count и retirement. Подтвердить data=RBP pre-state semantics, сравнение с direct ModRM RBP-base encoding (с учётом длины), последний disp32 byte меняет target, #SS start/end без callback, отдельный AddressOverflow, architectural/backend no-commit, committed store+STOP, entry guards, partial windows 1..4/1..7, invalid neighbors и dispatcher actual-RIP continuation через 5/8-byte forms без replay. Address32 sibling с `67`, ModRM `mod=01/10` и SIB `0x25` покрыт отдельным профилем S3-20cd; здесь проверяется только address64 форма. См. [S3-20cc report](../reports/STAGE_3_S3-20cc_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20cd. Address32 RBP-base SIB без индекса с displacement

Принимать только singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB ровно `0x25` (scale 1, no-index, RBP base), data RAX..RBP, Dword EA/Qword transfer, длины 6/9. ModRM `mod=00` с тем же SIB остаётся абсолютным no-base S3-20av и не смешивается с этим профилем. Адрес вычисляет decoder-owned `MemoryOperand::access_address_48`: `(low32(old RBP)+signed displacement) mod 2^32`; high32 RBP не участвует. Data=RBP использует старое значение для адреса/store payload; успешный load записывает полный qword в destination. Не добавлять scale aliases, иные индексы, prefixes, pairs или V3.

Приёмка: обе операции и все шесть data GPR по signed disp8/disp32 границам; direct V2/shared IR/interpreter parity полного CPU/RAM, independent EA oracle, callback address/width/count и retirement; сравнение с address32 direct-base encoding при учёте разной длины/RIP. Проверить high-half truncation и contiguous qword через 4 GiB, architectural/backend no-commit, store+STOP commit, entry STOP/budget-zero, disp32 final-byte target identity, partial windows 1..5/1..8, invalid neighbors и actual-RIP dispatcher continuation через 6/9-byte forms без replay. См. [S3-20cd report](../reports/STAGE_3_S3-20cd_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются за пределами среза.

#### S3-20ce. Address64 RSI/RDI-base SIB без индекса

Поддержать только singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB ровно `0x26` или `0x27` (scale 1, index отсутствует, base RSI/RDI), data RAX..RBP, qword EA/data и длины 4/5/8. Адрес задаёт decoder-owned `MemoryOperand::access_address_48`: старое полное значение RSI/RDI плюс signed displacement с wrapping u64. RSP не участвует в адресе и может быть data-регистром; полные значения RSI/RDI сохраняются. Не добавлять address32/`67`, scale aliases, другие базы/индексы, data вне RAX..RBP, пары, V3 или owner/MMU.

Приёмка: обе операции × RSI/RDI × все шесть data GPR × no-disp/disp8/disp32 формы и signed edges; direct V2/shared IR/interpreter full CPU/RAM/callback/retirement parity, независимый EA и сравнение с эквивалентным direct-base encoding с нормализацией next RIP. Проверить независимость от RSP, #GP на noncanonical start/end без callback включая RSP/RBP data, AddressOverflow, architectural/backend no-commit, точный store+STOP payload, entry guards, disp32 final-byte target identity, partial windows 1..3/1..4/1..7, invalid neighbors и actual-RIP dispatcher continuation через 4/5/8-byte формы без replay. См. отчёт [S3-20ce](../reports/STAGE_3_S3-20ce_2026-10-28.md). Mini и performance отложены до конца Stage 3; production owner/MMU и Stage 2/ISA остаются вне scope.

#### S3-20bv. Address32 indexed SIB с RBP base и RSI/RDI index

Поддержать только singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base ровно RBP, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, lengths 6/9. `mod=00` с SIB base field 5 — no-base и вне профиля. Effective address остаётся decoder-owned `MemoryOperand::access_address_48`: `(low32(old RBP)+low32(old RSI/RDI)*scale+signed displacement) mod 2^32`, затем zero-extension; transfer и store payload остаются qword. Data=RBP использует pre-state для адреса/store payload; load меняет RBP только после успешного read. RSI/RDI остаются index-only и неизменны.

Приёмка: direct V2/shared IR/interpreter parity обеих операций × шести data GPR × двух indices × четырёх scales × signed disp8/disp32 edges; full CPU/RAM, независимый EA oracle, callback address/length/count и retirement. Проверить high-half truncation/wrap, contiguous qword crossing 4 GiB, final disp32 byte target identity, architectural/backend no-commit для обоих направлений, store+STOP, dispatcher entry STOP/stop-RIP/zero-budget, partial windows 1..5 и 1..8, unsupported neighbors и actual-RIP continuation по 6/9-byte singleton без replay. Не расширять generic register whitelist и другие профили. Отчёт: [S3-20bv report](../reports/STAGE_3_S3-20bv_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bw. Address64 indexed no-base SIB с RSI/RDI index и disp32

Поддержать только singleton `48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (нет base), index RSI или RDI, scale 1/2/4/8, data RAX..RBP, точная длина 8. EA — старый полный индекс × scale + decoder-owned signed disp32 по модулю u64; base/RBP/RIP не участвуют. RSI/RDI доступны только как index и не меняются. Не принимать `67`, другие prefixes, другие index/base shapes, пары, V3 или owner/MMU интеграцию.

Приёмка: exact gates во всех compiler/IR/runtime/dispatcher layers; direct V2/shared IR/interpreter parity load/store × шесть data GPR × два indices × четыре scales × signed disp32 границы; независимый full-u64 EA oracle, CPU/RAM, callback address/count и retirement. Проверить высокие биты индекса и u64 wrapping, canonical start/noncanonical end #GP без callback, end-of-u64 overflow отдельно, architectural/backend faults без commit, committed store+STOP, entry STOP/stop-RIP/budget0, окна 1..7 без effects, изменение source byte 7 меняет target и bounded dispatcher continuation с actual RIP без replay. Сохранить no-base/no-index `0x25` и address32 no-base sibling как отдельные профили.

#### S3-20bx. Address32 indexed no-base SIB с RSI/RDI index и disp32

Поддержать только singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00/rm=100`, SIB base field `101` (нет base), index RSI или RDI, scale 1/2/4/8, data RAX..RBP, точная длина 9. EA остаётся decoder-owned: `(low32(index)*scale + signed disp32) mod 2^32`, затем zero-extension для qword transfer; base и RIP не участвуют, полные RSI/RDI сохраняются. Не расширять другие prefixes, index/data формы, пары или V3.

Приёмка: direct V2/shared IR/interpreter parity обеих операций × шести data GPR × RSI/RDI × четырёх scales × signed disp32 границ; полный CPU/RAM, независимый EA oracle, callback address/width/count и retirement. Проверить high-half truncation, modulo-2^32 product/add wrap, contiguous qword crossing 4 GiB, ninth-byte target identity, architectural/backend no-commit, store+STOP с точным payload, entry STOP/stop-RIP/budget0, partial windows 1..8 без effects, invalid neighbors и actual-RIP continuation через 9-byte singleton без replay. См. [S3-20bx report](../reports/STAGE_3_S3-20bx_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне scope.

#### S3-20by. Address64 indexed SIB с RSI/RDI base и RSI/RDI index

Поддержать только singleton `48 8B/89 /r sib`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI или RDI, index RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 4/5/8. EA использует полные старые значения обоих регистров: `old_base + old_index * scale + signed displacement` с wrapping по u64. Когда base и index кодируют один регистр, обе части выражения читают одно и то же pre-state значение и складываются отдельно. RSI/RDI доступны только как адресные регистры и сохраняются; generic register whitelist не расширяется.

Приёмка: descriptor/host load-store matrix по всем четырём base/index парам, scale, data GPR и трём длинам; direct V2/shared IR/interpreter parity состояния, RAM, callback и retirement. Проверить независимую формулу с base=index alias, full-width high bits и wrap, signed displacement края, disp32 byte 7 target identity, #GP на noncanonical start/end без helper, AddressOverflow отдельно, arch/backend no-commit, store+STOP с полным payload, entry guards, partial windows, invalid neighbors и actual-RIP dispatcher continuation через singleton→NOP→frozen pair без повтора. Отчёт: [S3-20by report](../reports/STAGE_3_S3-20by_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bz. Address32 indexed SIB с RSI/RDI base и RSI/RDI index

Поддержать только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base и index каждый RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. EA остаётся у decoder `MemoryOperand::access_address_48`: `(low32(old_base)+low32(old_index)*scale+signed displacement) mod 2^32`, затем zero-extend для qword access. Если base=index, оба слагаемых используют одно старое значение; полные RSI/RDI сохраняются, data переносится целиком.

Приёмка: direct V2/shared IR/interpreter parity обеих операций по всем четырём base/index парам, трём формам/длинам, четырём scales и шести data GPR с signed disp8/disp32 edges; full CPU/RAM, независимая EA, callback address/width/count и retirement. Проверить low32 high-half truncation, base=index и product/add wrap, contiguous qword crossing 4 GiB, ninth-byte target identity, architectural/backend no-commit, committed store+STOP, entry STOP/stop-RIP/budget0, partial windows 1..4/1..5/1..8, invalid neighbors, и actual-RIP continuation через 5/6/9-byte singleton→NOP→frozen pair без повторного эффекта. См. [S3-20bz report](../reports/STAGE_3_S3-20bz_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20cj. Address32 direct low-base qword MOV с RSI/RDI data

Поддержать ровно `67 48 8B/89 /r [disp8|disp32]`: ModRM `mod=00/01/10`, RSI/RDI data (`reg=6|7`), RAX/RCX/RDX/RBX base (`r/m=0..3`), Dword effective address и Qword transfer. Singleton lengths 4/5/8; всего 48 opcode/data/base/form сочетаний. EA остаётся decoder-owned: `(low32(old_base) + signed displacement) mod 2^32`. Полный qword data переносится целиком, base сохраняется. Не добавлять SIB, другие prefixes/REX, другие base/data registers, пары или V3. RSI/RDI `WriteGpr` разрешается только сразу после matching `CheckedLoad` с тем же temporary и exact Dword operand; generic reads/arithmetic не расширяются.

Приёмка: direct V2/shared IR/interpreter parity полной CPU/RAM, callback address/width/count, RIP и retirement; signed displacement boundaries, явные high-half truncation/wrap и qword transfer через границу 4 GiB; fault/backend atomicity, store+STOP, entry guards без memory effects, actual-RIP continuation через 4/5/8 без replay, final disp32 byte target, partial windows 1..3/1..4/1..7, forged descriptors и invalid neighbors. Сохранить frame 176 bytes, code cap 4096 и 10×MOV code size 3868. Mini/performance отложены до конца Stage 3, owner/MMU/Stage 2 вне этого среза. См. [S3-20cj report](../reports/STAGE_3_S3-20cj_2026-10-28.md).

#### S3-20ck. RIP-relative qword MOV с RSI/RDI data

Поддержать только singleton `48 8B/89 /r disp32`: ModRM `mod=00,r/m=101`, RSI/RDI data (`reg=6|7`), Qword EA/data и точную длину 7. Decoder-owned `AddressBase::NextRip` плюс signed disp32 задаёт EA; runtime не вычисляет RIP-relative адрес независимо. SIB, address override, extra prefixes, другие data registers, пары и V3 не входят в срез. RSI/RDI `WriteGpr` допускается только сразу после соответствующего `CheckedLoad` с тем же temporary и operand; generic RSI/RDI ReadGpr/arithmetic остаются запрещены.

Приёмка: direct V2/shared IR/interpreter parity обеих операций, четырёх кодировок и signed disp32 boundaries по полному CPU/RAM/flags, callback address/width/count, RIP и retirement. Проверить одинаковые bytes при разных guest RIP, byte-7 identity, forged descriptor/source rejection, invalid data/base/index/scale/size/length и partial windows 1..6 без admission/effects, noncanonical #GP и backend/architectural no-commit, entry guards, полный qword store+STOP ровно один раз и actual-RIP dispatcher continuation без replay. Сохранить frame 176 bytes, cap 4096 и 10×MOV size 3868. Mini/performance отложены до конца Stage 3; owner/MMU и Stage 2 вне этого среза. См. [S3-20ck report](../reports/STAGE_3_S3-20ck_2026-10-28.md).

#### S3-20cl. Address64 RSP-base SIB без индекса с RSI/RDI data

Поддержать ровно singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB только `0x24` (scale 1, index отсутствует, RSP base), data RSI/RDI (`reg=6|7`), qword EA/data, длины 4/5/8 — всего 12 opcode/data/displacement-form сочетаний. EA берётся из decoder-owned `MemoryOperand::access_address_48` и использует полное старое guest RSP плюс signed displacement; host SP и данные RSP не участвуют в вычислении адреса. Успешный transfer сохраняет полный qword; RSI/RDI load destination допускается только сразу после matching `CheckedLoad` с тем же temporary и operand. Общие `ReadGpr`, арифметика, префиксы, address32, другие SIB значения, пары и V3 не расширяются.

Приёмка: direct V2/shared IR/interpreter parity всех 12 форм по полному CPU/RAM, callback address/width/count и retirement; signed disp8/disp32 края, точный target последнего байта disp32, RSP noncanonical start/end как #SS без callback, RBX control как #GP, overflow/backend/architectural no-commit, один полный store перед STOP, entry guards, partial windows 1..3/1..4/1..7, actual-RIP dispatcher continuation через 4/5/8 без replay и invalid neighbors. Сохранить frame 176 bytes, code cap 4096 и 10×MOV regression size 3868. Mini/performance отложены до конца Stage 3; owner/MMU и Stage 2 вне среза. См. [S3-20cl report](../reports/STAGE_3_S3-20cl_2026-10-28.md).

#### S3-20cm. Address64 direct RBP-base qword MOV с RSI/RDI data

Ограниченный optional профиль: ровно восемь singleton форм `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, RSI/RDI data (`reg=6|7`), RBP base (`r/m=5`), qword EA/data, длины 4/7. `[RBP]` — disp8=0; `mod=00,r/m=5` остаётся RIP-relative. SIB, address32, другие регистры, пары и V3 вне среза. Decoder-owned effective address сохраняет signed displacement семантику. RSI/RDI `WriteGpr` остаётся сразу после matching CheckedLoad; generic reads/arithmetic не открываются.

Приёмка: direct V2/shared IR/interpreter parity обеих операций/data-регистров по полному CPU/RAM/callback/retirement; dispatcher admission, STOP от load/store callback, ровно одна retirement, partial windows 1..3/1..6, actual-RIP +4/+7 continuation без replay; signed edges, unaligned qword и data=RBP old-prestate; source-byte binding, final disp32-byte target identity, #SS для noncanonical RBP start/end без callback, RBX #GP, backend/architectural atomicity, entry guards и полный store+STOP ровно один раз. Сохранить frame 176, code cap 4096, regression 10×MOV=3868. Mini/performance отложены до конца Stage 3. S3-20cm не является gate завершения Stage 3 (S3-33). См. [S3-20cm report](../reports/STAGE_3_S3-20cm_2026-10-29.md).

#### S3-20cq. Exact aliased Dword store `89 00`

Добавить к существующему exact unprefixed `89 03` store только singleton `89 00` (`MOV [RAX],EAX`) через тот же typed IR, allocator и один четырёхбайтовый synchronous write helper. Effective address — полное старое RAX; payload — low32 того же старого RAX; RAX не меняется. Сохраняются отдельный Dword artifact/compile flavor и exact source/site descriptor provenance. Не добавлять других регистров, prefixes, SIB, displacement, пары, cache/SMC, dispatcher, owner или MMU integration.

Приёмка: ровно один write размером 4 с exact payload по адресу выше 4 GiB, unaligned адресу и crossing `0xffff_ffff`; соседние байты не меняются. Architectural/backend callback faults сохраняют точный payload и не commit-ят state/RAM; noncanonical RAX даёт точный #GP до callback. Entry STOP/budget zero не пишет; callback STOP после успешной записи возвращается с committed RIP `A+2` и retired=1 ровно один раз. PC map привязан к `A+2`: HelperCall/AfterHelper и commit state inadmissible для snapshot, helper error exit разрешает replay только для all-or-nothing отказа до commit. Отвергать изменённые source/descriptors, префиксы, SIB, displacement, другой ModRM, partial source и trailing bytes; сохранить `89 03` и qword regressions. См. [S3-20cq report](../reports/STAGE_3_S3-20cq_2026-11-01.md). Mini, Mac mini, Stage 2, production owner/MMU, cache, SMC, dispatcher и performance вне scope.

#### S3-20fg. Address64 RAX-base disp8 Dword store `89 40 ib`

Поддержать одну точную форму `89 40 ib`: unprefixed 3-byte Dword store `MOV [RAX + signext(disp8)], EAX`, Qword effective address, RAX base, no index и scale 1. EA — полный старый RAX плюс sign-extended disp8 по модулю `2^64`; payload — low32 старого RAX. Полный RAX, остальные GPR и RFLAGS сохраняются; успешный RIP — `A+3`. Отдельно whitelist-ить bytes для компилятора, `CheckedDwordStore` block builder и host publisher; binding включает exact bytes и metadata/site profile. Не расширять другие базы, индексы, размеры, prefixes, пары или V3.

Приёмка: все 256 disp8 source bytes связаны с отдельным site и PC-map boundary `A+3`; reject truncation, suffix, prefixes, неверные opcode/base/source и forged metadata. Host parity проверяет signed края, full RAX выше 4 GiB, unaligned transfer, wrap и нулевой EA, точные 4 bytes и соседние sentinels, сохранение flags/registers, noncanonical #GP(13) и `[u64::MAX-2, +4)` overflow без commit. Backend `MemoryError` и каждая entry guard не меняют CPU/RAM; повтор после них пишет по тому же адресу/данным, а callback STOP после commit не replay-ит store. Проверки Mini отложены до конца Stage 3; performance, production owner/MMU и Stage 2 вне scope. Реализация и проверки фиксируются в [S3-20fg report](../reports/STAGE_3_S3-20fg_2026-11-02.md).

#### S3-20fh. Address64 RAX-base disp32 Dword load `8B 80 id`

Поддержать ровно шестибайтовую unprefixed форму `8B 80 disp32`: `MOV EAX,[RAX+signext(disp32)]`, Qword effective address, RAX base, no index и scale 1. EA вычисляется из полного старого RAX до EAX writeback по модулю `2^64`; чтение — одна contiguous checked операция шириной 4, значение zero-extends в RAX, остальные GPR/RFLAGS сохраняются, успешный RIP — `A+6`. Отдельно разрешить exact byte pattern в compiler metadata и `CheckedDwordLoad` block whitelist, а len6 RAX профиль — в block/IR/runtime/source matcher; общий host publisher уже принимает шестибайтовые sites. Не менять addr32 `67 8B 80`, truncated REX.W, адресный override, forged RAX metadata для `8B 83` и qword direct profile.

Приёмка: привязать все четыре disp32 bytes, signed MIN/MAX/-1/0/1, destination/base/address-size/scale/index metadata и PC-map boundary `A+6`; отвергать truncation/suffix/prefix, другие opcode/base/destination/address forms. Host parity сравнивает interpreter/JIT CPU и RAM с full-width RAX выше 4 GiB, независимым effective address, нулевым extended результатом, сохранёнными GPR/flags и ровно одним четырёхбайтовым чтением. Проверить unaligned/negative/wrapping адреса, canonical-boundary #GP(13) без backend, `[u64::MAX-2,+4)` overflow, backend error и entry guards без commit с повтором по прежнему EA, callback STOP после commit без replay. Mini отложена до конца Stage 3; Stage 2, M1, production owner/MMU и performance вне этого среза. Статус и непроведённые проверки фиксируются в [S3-20fh report](../reports/STAGE_3_S3-20fh_2026-11-02.md).

Статус реализации на `8f5d9bb`: bounded runtime slice прошёл независимое Sol/high static review после JIT и host coverage fixes. Behavior tests, build, formatting, Cargo, Python, Mini и M1 не запускались; exact gates pending. ISSUE-045 без изменений; Stage 2 исключён; Mini/M1 отложены; Stage 3 остаётся partial.

#### S3-20fi. Address64 RAX-base disp32 Dword store `89 80 id`

Поддержать только шестибайтовую unprefixed форму `89 80 disp32`: `MOV [RAX+signext(disp32)],EAX`, Qword effective address, RAX base, no index и scale 1. EA вычисляется из полного старого RAX с signed disp32 по модулю `2^64`; payload — low32 того же старого RAX. Выполняется одна contiguous checked запись шириной 4; все GPR и RFLAGS сохраняются, успешный RIP — `A+6`. Другие ModRM/base/opcode forms, prefixes, SIB и соседние transfer widths не входят в slice.

Приёмка: exact binding всех шести source bytes и site metadata; signed disp32 boundaries, forged metadata, truncation/trailing bytes и unsupported neighbors. Host parity проверяет full-width RAX и low32 payload, interpreter/native CPU/RAM parity, ровно одну запись, неповреждённые соседние bytes, entry guards, fault/backend atomicity и retry, canonical #GP без callback, `[u64::MAX-2,+4)` overflow и committed callback STOP без replay. Runtime `2c54a0a` и host/JIT coverage `4098ac1` прошли Sol/high static review. Behavior tests/build/formatting/Cargo/Python не запускались; exact gates pending. ISSUE-045 без изменений; Stage 2 исключён; Mini/M1 отложены; Stage 3 остаётся partial. См. [S3-20fi report](../reports/STAGE_3_S3-20fi_2026-11-02.md).

#### S3-20fl. Address32 RAX-alias disp8 Dword load `67 8B 40 ib`

Поддержать ровно четырёхбайтовую форму `67 8B 40 ib`: `MOV EAX,[EAX+sign_extend(ib)]`, где `67` — единственный prefix, RAX выступает alias-base, а индекс отсутствует. Effective address вычисляется до writeback из low32 исходного RAX: `zero_extend((low32(RAX) + sign_extend(ib)) mod 2^32)`. Выполняется одно contiguous checked чтение шириной 4; результат zero-extends в RAX, остальные GPR/RFLAGS не меняются, успешный RIP — `A+4`. Оборачивается только начальный EA; transfer bytes после `0xffffffff` продолжаются выше 4 GiB без побайтного wrap. Сохранить fault/retry и callback STOP без replay. Не добавлять addr32 disp32 `67 8B 80 id`, SIB, другие destination/base формы или дополнительные prefixes.

Приёмка: точная compiler/decoded/IR/runtime/site admission и привязка всех 256 disp8 source bytes к своему site; forged metadata и unsupported source forms должны оставаться закрыты. JIT fixture фиксирует `A+4` PC-map boundary и distinct provenance с соседними supported forms. Host fixture проверяет signed disp8 и 32-bit start-address wrap от исходного EAX, zero-extension прочитанного dword, contiguous transfer через 4 GiB, неизменность остальных регистров/flags и fault/retry без частичного commit. Реализация, harness gates и результаты ревью зафиксированы в [S3-20fl report](../reports/STAGE_3_S3-20fl_2026-11-02.md). Runtime `51460e1` и harness `8e8a7af` прошли отдельные Sol/high статические ревью без замечаний. Behavior tests, builds, formatting, Cargo и Python harness checks не запускались. Mini отложена до конца Stage 3; M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне этого среза; Stage 3 остаётся partial.

#### S3-20fm. Address32 RAX-alias disp8 Dword store `67 89 40 ib`

Поддержать ровно четырёхбайтовую форму `67 89 40 ib`: `MOV [EAX+sign_extend(ib)],EAX`, где `67` — единственный prefix, RAX выступает alias-base, а индекс отсутствует. Effective address вычисляется из low32 исходного RAX: `zero_extend((low32(RAX) + sign_extend(ib)) mod 2^32)`. Payload — low32 того же исходного RAX, независимо от displacement. Выполняется одна contiguous checked запись шириной 4; полный RAX, остальные GPR и RFLAGS сохраняются, успешный RIP — `A+4`. Оборачивается только начальный EA; transfer bytes после `0xffffffff` продолжаются выше 4 GiB без побайтного wrap. Не добавлять addr32 disp32 `67 89 80 id`, SIB, другие source/base формы или дополнительные prefixes.

Приёмка: точные compiler metadata, `CheckedDwordStore` literal-byte gate, decoded/IR/runtime profiles, source/site matcher и host publisher whitelist; все 256 disp8 source bytes привязаны к своему site. Host fixture различает адрес и payload, проверяет signed displacement, wrap начального EA, zero payload, unaligned/zero/boundary starts, crossing 4 GiB, сохранность полного CPU/RAM, write fault/retry, entry guards и callback STOP без replay. Реализация и harness зафиксированы в [S3-20fm report](../reports/STAGE_3_S3-20fm_2026-11-02.md). Runtime `26eb0f4` и harness `a094bf5` закоммичены отдельно; оба прошли независимое Sol/high статическое ревью без замечаний. Ревью harness подтвердило реальные Rust `#[test]` identities, matching argv и счётчики. Проверки поведения, сборка, форматирование, Cargo и Python harness не запускались. Mini отложена до конца Stage 3; M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне этого среза; Stage 3 остаётся partial.

#### S3-20fn. Address32 RAX-alias disp32 Dword load `67 8B 80 id`

Поддержать ровно семибайтовую форму `67 8B 80 id`: `MOV EAX,[EAX+disp32]`, где `67` — единственный prefix, RAX выступает addr32 alias-base, а индекс отсутствует. Disp32 читается как signed little-endian `i32` и хранится sign-extended в metadata. Effective address вычисляется до writeback из low32 исходного RAX: `zero_extend((low32(RAX) + signed_disp32) mod 2^32)`. Выполняется одно contiguous checked чтение шириной 4; результат zero-extends в full RAX, остальные GPR/RFLAGS и RAM сохраняются, успешный RIP — `A+7`. Оборачивается только начальный EA; байты transfer после `0xffffffff` продолжаются выше 4 GiB без побайтного wrap. Не добавлять SIB, другие destination/base формы или дополнительные prefixes.

Приёмка: точные compiler metadata и `CheckedDwordLoad` literal-byte gate, decoded/IR/runtime addr32 RAX profiles и source/site matcher связывают все семь source bytes и signed disp32 с отдельным site; forged metadata и неподдерживаемые формы остаются закрыты. JIT fixture проверяет изменение каждого disp32 byte, sign-extended metadata, `A+7` PC-map boundary и distinct provenance относительно singleton, disp8, SIB и addr64 forms. Host fixture проверяет signed крайние значения, wrap начального адреса из исходного EAX при любых high32 RAX, zero-extension результата, contiguous read через 4 GiB, сохранность CPU/RAM, read fault и entry-guard retry, callback STOP без replay. При расширении точной byte admission сохранить source/site mismatch проверки для ранее rejected `67 8B 80 00000000` в singleton, disp8, SIB и addr64 fixture. Runtime и harness отражены в [S3-20fn report](../reports/STAGE_3_S3-20fn_2026-11-02.md). Runtime commits `d2eb210` and `b5a8952` passed Sol/high static review with the P1 finding closed; harness gates прошли Sol/high статическое ревью и сверены с фактическими Rust test identities. Поведенческие проверки, сборка, форматирование, Cargo и Python harness не запускались. Mini отложена до конца Stage 3; M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне этого среза; Stage 3 остаётся partial.

#### S3-20fo. Address32 RAX-alias disp32 Dword store `67 89 80 id`

Поддержать ровно семибайтовую форму `67 89 80 id`: `MOV [EAX+disp32],EAX`, где `67` — единственный prefix, RAX выступает addr32 alias-base, индекс отсутствует, а signed little-endian `i32` displacement sign-extends в metadata. Effective address: `zero_extend((low32(RAX) + signed_disp32) mod 2^32)`; payload — low32 исходного RAX. Выполняется одна contiguous checked запись шириной 4; полный RAX, остальные GPR и RFLAGS сохраняются, успешный RIP — `A+7`. Оборачивается только стартовый EA, байты записи за `0xffffffff` продолжаются выше 4 GiB без побайтного wrap. Не расширять load admission, SIB, другие операнды или owner/MMU.

Приёмка: отдельные exact compiler metadata и `CheckedDwordStore` literal-byte gate, decoded/IR/runtime profile, source/site matcher и host publisher whitelist; JIT связывает каждый из четырёх displacement bytes, sign-extended metadata и PC-map boundary `A+7`. Host fixture различает адрес и payload, проверяет signed крайние displacement, wrap начального EA в обе стороны, нулевой payload, unaligned/zero/boundary starts и crossing 4 GiB. Faults и entry guards сохраняют state/RAM и допускают retry; callback STOP после успешной записи не повторяет её. При exact byte admission сохранить source/site mismatch-проверки для `67 89 80 00000000` в addr64 RAX store, addr32 alias singleton и disp8 fixtures. Harness identities: `s3_34cz_ir_addr32_alias_disp32_dword_store_source_binding`, `s3_34cz_host_addr32_alias_disp32_dword_store_wrapped_original_eax_address_and_payload`; fail-closed self-test: `test_s3_34cz_addr32_alias_disp32_dword_store_gates_are_pinned_and_fail_closed`. После пары 278 gates (268 acceptance + 10 lifecycle), 296 local steps и 123 self-test names сверены с harness; фактические Rust test identities и argv подтверждены. Runtime `a186a55` и harness `e7f2dd6` прошли отдельные Sol/high статические ревью без замечаний. Behavior/build/fmt/Cargo/Python проверки, Mini и M1 regression отложены; Stage 2 и production owner/MMU вне среза.

#### S3-20fp. RCX-base Dword load singleton `8B 01`

Поддержать ровно два байта `8B 01`: `MOV EAX,[RCX]`, operand Dword и address size Qword, без prefix, displacement и SIB. Effective address — полный исходный RCX, без усечения до ECX. Выполняется одно contiguous checked чтение шириной 4; результат zero-extends в full RAX, RCX и остальные GPR, RFLAGS и RAM сохраняются, успешный RIP — `A+2`. Существующие canonical48 и range preflight, guest/backend faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять RCX disp8/disp32, addr32 alias, SIB, другие base/destination формы или prefixes.

Приёмка: отдельные exact compiler metadata и `CheckedDwordLoad` literal-byte gate, decoded/IR/runtime address64 profile и source/site matcher для `8B 01`; RAX/RCX supported singleton forms имеют разные source/site bindings. JIT fixture проверяет точные bytes, base RCX, Dword/Qword widths и PC-map boundary `A+2`, а forged fields и формы с prefix/displacement остаются закрыты. Host fixture различает полный RCX и low32 decoy, проверяет zero-extension результата, canonical upper-half и максимальный допустимый диапазон, preflight отказа для noncanonical/range-overflow адресов, read fault/retry, каждый entry guard с отдельным retry и callback STOP без повторного чтения. Rust identities: `memory_load_v2::tests::ir_dword_rcx_base_load_binds_exact_source_and_site` и `tests::s3_20fp_rcx_base_dword_load_uses_full_rcx_and_zero_extends_eax`. Harness identities: `s3_34da_ir_rcx_base_dword_load_source_binding`, `s3_34da_host_rcx_base_dword_load_full_rcx_and_zero_extended_eax`; fail-closed self-test: `test_s3_34da_rcx_base_dword_load_gates_are_pinned_and_fail_closed`. После пары ожидаются 280 Rust gates (270 acceptance + 10 lifecycle), 298 local steps и 124 self-test names. Runtime commits `a99abbb`, `88eeae4`, `4b65abf` и `5bf4cd1` прошли Sol/high review; все P1/P2 findings закрыты. Harness correction commit `32cf793` исправил 71 multiline acceptance-count assertions с 268 на 270 и прошёл повторное Sol/high review. Поведенческие проверки, сборка, форматирование, Cargo и Python harness не запускались. Mini отложена до конца Stage 3; M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне среза; Stage 3 остаётся partial. См. [S3-20fp report](../reports/STAGE_3_S3-20fp_2026-11-02.md).

#### S3-20fq. RCX-base Dword store singleton `89 01`

Поддержать ровно два байта `89 01`: `MOV [RCX],EAX`, operand Dword и address size Qword, без prefix, displacement и SIB. Effective address — полный исходный RCX; payload — low32 исходного RAX. Выполняется одна contiguous checked запись шириной 4; полный RAX, RCX, остальные GPR и RFLAGS сохраняются, успешный RIP — `A+2`. Существующие canonical48/range preflight, guest/backend faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять RCX disp8/disp32, addr32 alias, SIB, другие base/source формы или prefixes.

Приёмка: отдельные exact compiler metadata и `CheckedDwordStore` literal-byte gate, decoded/IR/runtime address64 profile, source/site matcher и host publisher whitelist для `89 01`; поддержанные `89 00`/`89 03` должны сохранять разные bindings. JIT fixture проверяет точные bytes, source0, baseRCX, Dword/Qword widths и PC-map boundary `A+2`; forged fields и соседние неподдерживаемые формы остаются закрыты. Host fixture различает полный RCX и low32 decoy, независимо проверяет low32 RAX payload, zero/unaligned адреса, canonical upper-half и максимальный допустимый диапазон, preflight отказа для noncanonical/canonical-boundary/range-overflow адресов, write fault с retry, каждый entry guard с отдельным retry и callback STOP без replay.

Rust identities: `memory_load_v2::tests::ir_dword_rcx_base_store_binds_exact_source_and_site` и `tests::s3_20fq_rcx_base_dword_store_uses_full_rcx_and_low_eax_payload`. Harness identities: `s3_34db_ir_rcx_base_dword_store_source_binding`, `s3_34db_host_rcx_base_dword_store_full_rcx_and_low_eax_payload`; fail-closed self-test: `test_s3_34db_rcx_base_dword_store_gates_are_pinned_and_fail_closed`. Итоговые totals — 282 Rust gates (272 acceptance + 10 lifecycle), 300 local steps и 125 self-test names. Runtime commits `f6744bf`, `cc5f85a` и harness commit `c7cb7c1` прошли независимое Sol/high review. Harness identity correction `cac3a67` закрыл P1: Rust test identity согласован с pinned gate; повторное review прошло. Поведенческие проверки, сборка, форматирование, Cargo и Python harness отложены до завершения этапа; Mini также отложена до конца Stage 3, M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне среза; Stage 3 остаётся partial. См. [S3-20fq report](../reports/STAGE_3_S3-20fq_2026-11-02.md).

#### S3-20fr. RCX-base Dword load signed disp8 `8B 41 ib`

**Статус: PASS.**

Поддержана ровно трёхбайтовая форма `8B 41 ib`: `MOV EAX,[RCX+sign_extend(ib)]`, operand Dword и address size Qword, без prefix, index или SIB. Effective address — полный исходный RCX с signed disp8, сложенным modulo 2⁶⁴; ECX не используется. Выполняется одно contiguous checked чтение шириной 4; результат zero-extends в full RAX, RCX и остальные GPR, RFLAGS и RAM сохраняются, успешный RIP — `A+3`. Canonicality и range проверяются для вычисленного EA и полного диапазона чтения. Существующие guest/backend faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять RCX disp32, addr32, RDX-base, другие destinations или SIB.

Приёмка: exact compiler metadata, отдельный `CheckedDwordLoad` literal-byte gate, decoded/IR/runtime length3/i8 profile и source/site matcher для `[8B,41,ib]`. Сохранить поддержку `[8B,00]`/`[8B,03]` как разных source/site forms. JIT fixture проверяет все 256 displacement bytes, signed metadata, forged site fields и PC-map boundary `A+3`; неподдерживаемые prefixes, destinations, displacement widths и addressing forms остаются закрыты. Host fixture проверяет signed displacement, полный RCX против low32 decoy, нулевой и unaligned EA, canonical upper-half, арифметику вокруг `u64::MAX`, noncanonical итоговый EA и overflow полного диапазона без вызова read callback, guest/backend fault и retry, отдельный retry для каждого entry guard и callback STOP без replay.

Rust identities: `memory_load_v2::tests::ir_dword_rcx_disp8_load_binds_all_signed_source_bytes_to_its_site` и `tests::s3_20fr_rcx_disp8_dword_load_uses_full_rcx_and_signed_displacement`. Harness identities: `s3_34dc_ir_rcx_disp8_dword_load_source_binding`, `s3_34dc_host_rcx_disp8_dword_load_full_rcx_and_signed_displacement`; fail-closed self-test: `test_s3_34dc_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed`. Итоговые totals — 284 Rust gates (274 acceptance + 10 lifecycle), 302 local steps и 126 self-test names; identities and argv сверены с фактическими Rust test definitions. Runtime commits `b23f01b` и `0f2d87c` и harness commit `2efd760` прошли независимое Sol/high review. Follow-up `0f2d87c` исправил прямое распространение ошибки чтения displacement byte в host fixture.

Поведенческие проверки, сборка, форматирование, Cargo и Python harness отложены до завершения этапа; Mini отложена до конца Stage 3, M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне среза; Stage 3 остаётся partial. См. [S3-20fr report](../reports/STAGE_3_S3-20fr_2026-11-02.md).

#### S3-20fs. RCX-base Dword store signed disp8 `89 41 ib`

Поддержать ровно три байта `89 41 ib`, без prefixes: `MOV [RCX+sign_extend(ib)],EAX`, operand Dword, address size Qword, base RCX, no index, scale 1. Disp8 sign-extends в i64; EA — полный исходный RCX плюс displacement modulo 2⁶⁴. Payload — low32 исходного RAX. Выполняется одна contiguous checked запись шириной 4; полный RAX, RCX, остальные GPR и RFLAGS сохраняются, успешный RIP — `A+3`. Canonicality и range проверяются для вычисленного EA и полного диапазона записи. Существующие faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять RCX disp32, addr32, SIB, другие базы или source registers.

Приёмка: exact compiler metadata, отдельный `CheckedDwordStore` literal-byte gate, decoded/IR/runtime length3/i8 Store profile, source/site matcher и host publisher whitelist для `[89,41,ib]`. JIT fixture должен связать все 256 disp8 значений с отдельным site, проверить signed metadata, forged fields и PC-map boundary `A+3`; соседние и неподдерживаемые формы остаются закрыты. Host fixture должен различать полный RCX и low32 decoy, отдельно проверять low32 RAX payload, нулевой/unaligned EA, upper canonical half, wrap у `u64::MAX`, noncanonical итоговый EA и overflow полного диапазона без вызова write callback, guest/backend write failure и retry, отдельный successful retry после каждого entry guard и callback STOP без replay. Ошибка чтения displacement byte при получении инструкции должна распространяться напрямую, без подстановочного байта или fallback.

Фактические Rust identities: `memory_load_v2::tests::ir_dword_rcx_disp8_store_binds_all_signed_source_bytes_to_its_site` и `tests::s3_20fs_rcx_disp8_dword_store_uses_full_rcx_and_low_eax_payload`. Harness identities: `s3_34dd_ir_rcx_disp8_dword_store_source_binding`, `s3_34dd_host_rcx_disp8_dword_store_full_rcx_and_low_eax_payload`; fail-closed self-test: `test_s3_34dd_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed`. Фактические totals — 286 Rust gates (276 acceptance + 10 lifecycle), 304 local steps и 127 self-test names; identities и argv сверены с реализацией и harness. Runtime commits `11fe79a`, `2ef42ef` и `b0fd30e`, а также harness commit `0027a03` прошли независимое Sol/high review (PASS). Host fixture follow-ups `2ef42ef` и `b0fd30e` исправили захват helper closure и её синтаксическое завершение. Ошибка чтения displacement byte должна распространяться напрямую, без подстановочного значения или fallback.

Поведенческие проверки, сборка, форматирование, Cargo и Python harness отложены до завершения этапа; Mini отложена до конца Stage 3, M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне среза; Stage 3 остаётся partial. См. [S3-20fs report](../reports/STAGE_3_S3-20fs_2026-11-02.md).

#### S3-20ft. RCX-base Dword load signed disp32 `8B 81 id`

**Статус: PASS.**

Поддержать ровно шестибайтовую форму `8B 81 id`: `MOV EAX,[RCX+disp32]`, operand Dword и address size Qword, без prefix, index или SIB. Disp32 читается как signed little-endian `i32` и sign-extends в metadata. Effective address вычисляется из полного исходного RCX: `original_RCX.wrapping_add_signed(displacement)` modulo 2⁶⁴. Выполняется одно contiguous checked чтение шириной 4; результат zero-extends в full RAX, RCX и остальные GPR, RFLAGS и RAM сохраняются, успешный RIP — `A+6`. Canonicality и range preflight применяются к вычисленному EA и полному диапазону чтения. Существующие guest/backend faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять addr32, другие bases/destinations, prefixes или SIB.

Приёмка: exact compiler metadata, отдельный `CheckedDwordLoad` literal-byte gate, decoded/IR/runtime length6/i32 profile и source/site matcher для `[8B,81,id]`. Сохранить соседние формы как разные sites и отвергать malformed/неподдерживаемые формы. JIT fixture связывает каждый из четырёх displacement bytes со своим site, проверяет signed metadata, forged fields и PC-map boundary `A+6`; три текущих compile-negative с `[8B,81,id]` должны стать успешной компиляцией с двусторонним source/site mismatch. Host fixture различает полный RCX и low32 decoy, проверяет signed displacement и full64 арифметику, нулевой/unaligned EA, canonical upper-half, допустимый диапазон около `u64::MAX`, noncanonical/range failures до callback, guest/backend faults с retry, отдельный retry каждого entry guard и callback STOP без replay. Ошибка чтения любого displacement byte при fetch инструкции должна распространяться напрямую, без подстановочного значения или fallback.

Фактические Rust identities: `memory_load_v2::tests::ir_dword_rcx_disp32_load_binds_all_source_bytes_to_its_site` и `tests::s3_20ft_rcx_disp32_dword_load_uses_full_rcx_and_signed_displacement`. Harness identities: `s3_34de_ir_rcx_disp32_dword_load_source_binding`, `s3_34de_host_rcx_disp32_dword_load_full_rcx_and_signed_displacement`; fail-closed self-test: `test_s3_34de_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed`. Итоговые totals — 288 gates (278 acceptance + 10 lifecycle), 306 local steps и 128 self-test names; identities и argv сверены с интегрированным harness. Runtime commits `dea7edf` и `74da03a`, а также harness commit `3c30863` интегрированы и прошли независимое Sol/high review (PASS). В JIT neighbor fixture использована точная поддержанная форма `[8B,01]`. Широкие проверки этапа, сборка, форматирование, Cargo и полный Python harness отложены до конца этапа; Mini и M1 regression также отложены по решениям пользователя. Stage 2 и production owner/MMU вне scope; Stage 3 остаётся partial.

См. [S3-20ft report](../reports/STAGE_3_S3-20ft_2026-11-02.md).

#### S3-20fu. RCX-base Dword store signed disp32 `89 81 id`

**Статус: PASS.**

Поддержать ровно шестибайтовую форму `89 81 id`: `MOV [RCX+disp32],EAX`, operand Dword и address size Qword, без prefix, index или SIB. Disp32 читается как signed little-endian `i32` и sign-extends в site metadata. Effective address — полный исходный RCX с signed displacement, сложенный modulo 2⁶⁴. Payload — low32 исходного RAX. Выполняется одна contiguous checked запись шириной 4; все GPR, включая полный RAX и RCX, и RFLAGS сохраняются; успешный RIP — `A+6`. Canonicality и range preflight применяются к вычисленному EA и полному диапазону записи. Существующие guest/backend faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять addr32, SIB, другие bases/source registers или prefixes.

Приёмка: exact compiler metadata, отдельный `CheckedDwordStore` literal-byte gate, decoded/IR/runtime length6/i32 Store profile, source/site matcher и host Store publisher whitelist для `[89,81,id]`. JIT fixture связывает каждый из четырёх displacement bytes со своим site, проверяет signed metadata, forged fields и PC-map boundary `A+6`; поддержанные формы должны оставаться разными sites. Две существующие compile-negative fixture с `[89,81,00000000]` — в RAX disp32 store и RCX singleton store — стали успешной компиляцией с двусторонним source/site mismatch. Сохранены rejection Store opcode в Load compiler и `[67 89 81 ...]` addr32 ECX-base negative. Host fixture различает полный RCX и low32 decoy, независимо проверяет low32 RAX payload, zero/unaligned EA, canonical upper-half, wrap и допустимый диапазон около `u64::MAX`, preflight failures без write callback, guest/backend fault с retry, отдельный retry каждого entry guard и callback STOP без replay. Ошибка чтения любого displacement byte распространяется без подстановки или fallback.

Фактические Rust identities: `memory_load_v2::tests::ir_dword_rcx_disp32_store_binds_all_source_bytes_to_its_site` и `tests::s3_20fu_rcx_disp32_dword_store_uses_full_rcx_and_low_eax_payload`. Harness gates: `s3_34df_ir_rcx_disp32_dword_store_source_binding`, `s3_34df_host_rcx_disp32_dword_store_full_rcx_and_low_eax_payload`; fail-closed self-test: `test_s3_34df_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed`. До среза harness содержал 288 gates (278 acceptance + 10 lifecycle), 306 local steps и 128 self-test names; после — 290 gates (280 acceptance + 10 lifecycle), 308 steps и 129 self-test names. Пять устаревших harness count pins исправлены. Runtime commits `08d3211`, `96b8ea9`, `881b9aa`, `277fedf` и harness commits `e9ed362`, `b4b8901` интегрированы; compile/type/source-boundary issues закрыты. Runtime и harness прошли отдельные независимые Sol/high review: PASS.

Behavior checks, сборка, форматирование, Cargo и Python harness отложены до завершения этапа; Mini — до конца Stage 3, M1 regression — по решению пользователя. Stage 2 и production owner/MMU вне scope; Stage 3 остаётся partial. См. [S3-20fu report](../reports/STAGE_3_S3-20fu_2026-11-02.md).

#### S3-20fv. Address32 RCX-base Dword load singleton `67 8B 01`

**Статус: PASS.**

Поддержать ровно три байта `67 8B 01`: `MOV EAX,[ECX]`, где `67` — единственный prefix, data и address sizes — Dword, base — addr32 alias RCX, индекс и displacement отсутствуют. Effective address — zero-extended low32 исходного RCX; high32, включая неканонические биты полного значения, не участвуют. Выполняется одно contiguous checked чтение шириной 4; результат zero-extends в полный RAX, полный RCX, остальные GPR и RFLAGS сохраняются, успешный RIP — `A+3`. Начальный адрес в диапазоне 32-bit; transfer bytes после `0xffffffff` продолжаются выше 4 GiB без побайтного wrap. Существующие guest/backend faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять RCX disp8/disp32, SIB, другие базы/destinations или дополнительные prefixes.

Приёмка: отдельные exact compiler metadata и `CheckedDwordLoad` literal-byte gate, отдельные decoded block/IR addr32 RCX length3/disp0 profiles, runtime site/source matcher для `[67,8B,01]` и существующий host publisher. Не добавлять RCX в общие RAX/RBX addr32 profiles. Поддержанные `[8B,01]`, `[67,8B,00]`, `[67,8B,03]` и `[67,8B,04,0B]` сохраняют distinct positive source/site bindings; последние три формы не являются negative cases. JIT fixture проверяет exact source, widths, base/index/scale/displacement и PC-map boundary `A+3`, forged metadata и neighboring supported sites. Host fixture различает одинаковый ECX при разных high32 RCX (включая неканонические full64 значения), проверяет low32 target против full64 decoy, zero/unaligned EA, starts `FFFFFFFD/FE/FF` с contiguous read через 4 GiB, payloads `0/80000000/FFFFFFFF`, полный RCX и zero-extension результата. Существующий stale negative `[67,8B,01]` в RCX-base singleton fixture переведён в успешную компиляцию с двусторонним source/site mismatch относительно `[8B,01]`; negatives сохранены для `[67,8B,41 ib]`, `[67,8B,81 id]`, repeated `67`, REX/operand-size/segment/LOCK, другой destination/base и indexed SIB. Сохраняются propagation fetch errors, lifetime compiled block/`.site()` и последовательные MAP_JIT publications.

Rust identities: `memory_load_v2::tests::ir_dword_addr32_rcx_base_load_binds_exact_source_and_site` и `tests::s3_20fv_addr32_rcx_base_dword_load_uses_zero_extended_ecx`. Harness identities: `s3_34dg_ir_addr32_rcx_base_dword_load_source_binding`, `s3_34dg_host_addr32_rcx_base_dword_load_zero_extended_ecx`; fail-closed self-test: `test_s3_34dg_addr32_rcx_base_dword_load_gates_are_pinned_and_fail_closed`. Totals after integration: 292 Rust gates (282 acceptance + 10 lifecycle), 310 local steps и 130 self-test names. Runtime commits `a93aaa3`, `5d4cd11`, `7bc63dc` and harness commit `e3c9020` интегрированы; runtime и harness прошли независимое Sol/high review (PASS). Проверки исполнения, сборка, форматирование, Cargo, Python harness и Mini отложены до конца Stage 3; M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне scope; Stage 3 остаётся partial. См. [S3-20fv report](../reports/STAGE_3_S3-20fv_2026-11-02.md).
#### S3-20fw. Address32 RCX-base Dword store singleton `67 89 01`

**Статус реализации и независимого Sol/high review: PASS; проверки исполнения и сборки отложены.**

Поддержать ровно три байта `67 89 01`: `MOV [ECX],EAX`, где `67` — единственный prefix, data и address sizes — Dword, base — addr32 alias RCX, индекс и displacement отсутствуют. Effective address — zero-extended low32 исходного RCX; high32 полного значения не участвуют. Payload — low32 исходного RAX. Выполняется одна contiguous checked запись шириной 4; полный RAX, RCX, остальные GPR и RFLAGS сохраняются, успешный RIP — `A+3`. Начальный адрес в диапазоне 32-bit; transfer bytes после `0xffffffff` продолжаются выше 4 GiB без побайтного wrap. Существующие guest/backend faults, entry guards, retry и callback STOP без replay сохраняются. Не расширять addr32 RCX disp8/disp32, SIB, другие bases/sources или дополнительные prefixes.

Приёмка: exact compiler metadata, отдельный `CheckedDwordStore` literal-byte gate, узкий addr32 RCX singleton predicate в decoded block profile и его включение во все три store admission groups, отдельный typed IR predicate и включение в store address union, runtime site/source matcher и host publisher whitelist. RCX нельзя добавлять целиком в общие RAX/RBX addr32 profiles: они допускают другие displacement/SIB формы. Конвертировать stale negatives `[67,89,01]` в RAX-alias и RCX addr64 singleton fixtures в successful compile с двусторонним source/site mismatch. Сохранить `[67,89,01,80]` как trailing-byte negative и остальные unsupported prefix/operand/SIB формы.

JIT fixture закрепляет exact bytes, widths, base/index/scale/displacement, PC-map boundary `A+3`, forged metadata и distinct positive bindings для поддержанных соседних source forms. Host fixture варьирует high32 RCX при одинаковом ECX (включая неканонические high bits полного исходного RCX, которые игнорируются именно при вычислении addr32 EA), отличает low32 target от full64 decoy, проверяет zero/unaligned/boundary starts `FFFFFFFD/FE/FF`, contiguous write через 4 GiB, независимый payload RAX, payload edges, fault/guard retries и callback STOP без replay. Ошибки fetch должны распространяться напрямую.

Фактические Rust identities: `memory_load_v2::tests::ir_dword_addr32_rcx_base_store_binds_exact_source_and_site` и `tests::s3_20fw_addr32_rcx_base_dword_store_uses_zero_extended_ecx_and_low_eax_payload`. Harness identities: `s3_34dh_ir_addr32_rcx_base_dword_store_source_binding`, `s3_34dh_host_addr32_rcx_base_dword_store_zero_extended_ecx_and_low_eax_payload`; fail-closed self-test: `test_s3_34dh_addr32_rcx_base_dword_store_gates_are_pinned_and_fail_closed`. Runtime commits `8fa1400` и `6dbceee`, а также harness commit `4cc94ba` интегрированы и прошли независимое Sol/high review (PASS). Итоговые totals — 294 Rust gates (284 acceptance + 10 lifecycle), 312 local steps и 131 self-test names. Behavior/build/fmt/Cargo/Python checks are deferred to the end; Mini is deferred until the end of Stage 3 and M1 regression is deferred by the user. Stage 2 and production owner/MMU remain outside scope; Stage 3 is partial. См. [S3-20fw report](../reports/STAGE_3_S3-20fw_2026-11-02.md).

#### S3-20fx. Address32 RCX-base Dword load signed disp8 `67 8B 41 ib`

**Статус реализации и независимого Sol/high review: PASS; этапные проверки отложены.**

Поддержать ровно четыре байта `67 8B 41 ib`: `MOV EAX,[ECX+signext(disp8)]`, где `67` — единственный prefix, data и address sizes — Dword, base — addr32 alias RCX, индекс отсутствует, scale равен 1, displacement занимает ровно один байт и трактуется как signed i8. Начальный effective address — zero-extended результат `(original_ECX + sign_extend(ib)) mod 2^32`; полный исходный RCX сохраняется. Выполняется одно contiguous checked чтение шириной 4, значение zero-extends в полный RAX, остальные GPR и RFLAGS сохраняются, успешный RIP — `A+4`. Только вычисление начального адреса использует 32-bit wrap; transfer bytes после `0xffffffff` идут выше 4 GiB без побайтного wrap. Существующие memory/backend faults, entry guards, retry и callback STOP без replay сохраняются.

Не расширять срез на RCX disp32 (`67 8B 81 id`), indexed SIB, другие bases/destinations или дополнительные prefixes. Поддержанные `[67 8B 01]`, `[8B 41 ib]`, `[67 8B 40 ib]` и `[67 8B 43 ib]` остаются distinct positive source/site forms. Не превращать корректные соседние формы в negatives.

Приёмка должна связать точные четыре source bytes с отдельным addr32 RCX-base Dword disp8 site и typed load metadata; JIT fixture покрывает все 256 значений `ib`, signed displacement, source/site mismatch, forged поля и PC-map boundary `A+4`. Host fixture проверяет signed disp8 и wrap адреса по ECX, сохранение полного RCX, contiguous checked dword read, zero-extension в RAX, guard/fault/retry и STOP без replay. Старые negatives `[67 8B 41 00]` в addr32 RCX singleton fixture и `[67 8B 41 80]` в addr64 RCX disp8 fixture должны стать успешной компиляцией с двусторонним source/site mismatch; сохранить исключения для disp32, SIB/index, неподдержанных prefixes/destinations/bases, лишних и усечённых байтов.

Фактические Rust identities: `memory_load_v2::tests::ir_dword_addr32_rcx_disp8_load_binds_all_signed_source_bytes_to_its_site` и `tests::s3_20fx_addr32_rcx_disp8_dword_load_uses_wrapped_ecx`. Harness gate identities: `s3_34di_ir_addr32_rcx_disp8_dword_load_source_binding`, `s3_34di_host_addr32_rcx_disp8_dword_load_wrapped_ecx`; fail-closed self-test: `test_s3_34di_addr32_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed`. Baseline before this slice: 294 Rust gates (284 acceptance + 10 lifecycle), 312 local steps и 131 self-test names; after integration: 296 gates (286 acceptance + 10 lifecycle), 314 steps и 132 self-test names. Runtime commits `930e4c5` и `ce0701d`, harness commit `c5224aa` интегрированы; runtime и harness прошли независимое Sol/high review (PASS). В host fixture исходный RCX равен `0x1234567800000100`; full64 decoy start EA равен `RCX−128`, то есть `0x1234567800000080`, и не совпадает с проверяемым low32 target. Проверки исполнения, сборка, форматирование, Cargo и Python harness отложены до конца Stage 3.

Local behavior checks passed: full od-jit suite 397/397, exact host regression, and fail-closed S3-34di Python self-test. The final Graphify refresh timed out and remains unconfirmed. Stage-wide checks are deferred until the end of Stage 3; Mini is deferred until the end of Stage 3 and M1 regression is deferred by the user. Stage 2 and production owner/MMU integration remain outside scope; Stage 3 remains partial. См. [S3-20fx report](../reports/STAGE_3_S3-20fx_2026-11-02.md).

#### S3-20fy. Address32 RCX-base Dword store signed disp8 `67 89 41 ib`

**Статус реализации и статического независимого Sol/high review: PASS; этапные проверки отложены.**

Поддержать ровно четыре байта `67 89 41 ib`: `MOV [ECX+signext(disp8)],EAX`, где `67` — единственный prefix, data и address sizes — Dword, base — addr32 alias RCX, индекс отсутствует, scale равен 1, displacement занимает один байт и трактуется как signed i8. Начальный effective address — zero-extended результат `(original_ECX + sign_extend(ib)) mod 2^32`; полный исходный RCX сохраняется. Выполняется одна contiguous checked запись шириной 4 с low32 исходного RAX; остальные GPR и RFLAGS сохраняются, успешный RIP — `A+4`. Только вычисление начального адреса использует 32-bit wrap: transfer bytes после `0xffffffff` идут выше 4 GiB без побайтного wrap. Существующие memory/backend faults, entry guards, retry и callback STOP без replay сохраняются.

Не расширять срез на RCX disp32, indexed SIB, другие bases/sources или дополнительные prefixes. Поддержанные addr32 RCX singleton store `67 89 01` и disp8 load/store соседние формы должны оставаться distinct source/site bindings. Сохранить exact trailing-byte negative для `[67,89,41,ib,extra]` и truncated-source negatives; не принимать лишний байт как часть формы.

Rust identities: `memory_load_v2::tests::ir_dword_addr32_rcx_disp8_store_binds_all_signed_source_bytes_to_its_site` и `tests::s3_20fy_addr32_rcx_disp8_dword_store_uses_wrapped_ecx_and_low_eax_payload`. Harness gates: `s3_34dj_ir_addr32_rcx_disp8_dword_store_source_binding`, `s3_34dj_host_addr32_rcx_disp8_dword_store_wrapped_ecx_and_low_eax_payload`; fail-closed self-test: `test_s3_34dj_addr32_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed`. Expected manifest totals after harness integration: 298 gates (288 acceptance + 10 lifecycle), 316 local steps, 133 self-test names. These are target counts, not verified current evidence.

Приёмка связывает exact source bytes с отдельным RCX addr32 disp8 site в compiler, отдельном `CheckedDwordStore` literal-byte gate, decoded/IR/runtime profile, source/site matcher и host Store publisher whitelist. JIT fixture проверяет все 256 значений `ib`, signed metadata, bidirectional source/site mismatch относительно `[67 89 01]` и PC-map boundary `A+4`. Host fixture для каждого displacement проверяет full-RCX/low32 decoys, low32 payload, zero/unaligned адреса, wrap в обе стороны, старты `0xfffffffd…0xffffffff`, contiguous переход через 4 GiB, demand-fetch ошибки в interpreter path, fault/retry, каждую entry guard/retry и STOP без replay. Published IR artifact использует immutable compiler-issued source и не fetch-ит guest instruction bytes во время runtime execution. Устаревший negative `[67 89 41 00]` в singleton fixture стал успешной компиляцией с двусторонней source/site проверкой; trailing-byte negatives сохранены.

Статическое независимое Sol/high review выполнено: PASS. Проверки, сборка, форматирование, Cargo, Python harness и Mini не запускались и отложены до конца Stage 3; M1 regression отложена по решению пользователя. Published IR runtime использует immutable compiler-issued source и не fetch-ит guest instruction bytes; ошибки fetch проверяются demand interpreter path. Stage 2 и production owner/MMU вне scope, Stage 3 остаётся partial. См. [S3-20fy report](../reports/STAGE_3_S3-20fy_2026-11-02.md) и [S3-34dj report](../reports/STAGE_3_S3-34dj_2026-11-02.md).

#### S3-20fz. Address32 RCX-base Dword load signed disp32 `67 8B 81 id`

**Статус реализации и независимого статического review: PASS; поведенческие проверки отложены.** Runtime commits `ac69bcd` и `baba8f8` интегрированы.

Поддержать только семь байт `67 8B 81 id`: `MOV EAX,[ECX+signext(disp32)]`; `67` — единственный prefix, data/address size — Dword, база — addr32 alias RCX, индекс отсутствует, scale равен 1, смещение — signed little-endian i32. Effective address вычисляется как `zero_extend((low32(RCX) + signed_i32_disp32) mod 2^32)`. Выполнить одну contiguous checked read шириной четыре байта; zero-extend результата в полный RAX. RCX, остальные GPR и RFLAGS сохраняются; успешный RIP — `A+7`. Только вычисление начального EA оборачивается по 32 битам: байты transfer после `0xffffffff` продолжаются выше 4 GiB без побайтного wrap. Сохранить memory/backend fault, entry-guard, retry и callback STOP после успешного commit без повторного чтения. Ошибки instruction fetch проверяются demand interpreter path; published IR runtime использует immutable compiler-issued source.

Scope исключает RCX disp32 store, любую другую базу, SIB/index, дополнительные prefixes/REX и прочие формы. Поддержанные соседи, включая `67 8B 01` и `67 8B 41 ib`, обязаны иметь distinct source/site bindings. Сохранить rejection truncated/trailing source и неподдержанных соседей.

Фактические Rust identities: `memory_load_v2::tests::ir_dword_addr32_rcx_disp32_load_binds_exact_source_and_site` и `tests::s3_20fz_addr32_rcx_disp32_dword_load_uses_wrapped_ecx`. Harness gates: `s3_34dk_ir_addr32_rcx_disp32_dword_load_source_binding`, `s3_34dk_host_addr32_rcx_disp32_dword_load_wrapped_ecx`; fail-closed self-test: `test_s3_34dk_addr32_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed`. Harness commit `2de1cab` и runtime commits `ac69bcd`/`baba8f8` интегрированы и прошли независимое статическое Sol/high review.

Приёмка должна закрепить точные source bytes и signed metadata в compiler/decoded/IR/runtime/site binding; проверить отсутствие alias с `67 8B 01` и `67 8B 41 ib`, exact PC-map boundary `A+7`, forged metadata и truncation/trailing negatives. Host coverage должна сравнить full RCX и low32 decoys, signed disp32 edges, wrap начального адреса, contiguous checked read через 4 GiB, zero-extension в RAX, сохранность остальных GPR/RFLAGS/RAM, canonical/range/backend faults без commit, retry по исходному EA, entry guards и callback STOP без replay.

Manifest totals 300 gates (290 acceptance + 10 lifecycle), 318 local steps и 134 self-test names закреплены и прошли статическое ревью; исполнения не было. Behavior tests, build, formatting, Cargo, Python harness и Mini не запускались и отложены. M1 regression отложена по решению пользователя. Stage 2 и production owner/MMU вне scope; Stage 3 остаётся partial. См. [S3-20fz report](../reports/STAGE_3_S3-20fz_2026-11-02.md) и [S3-34dk report](../reports/STAGE_3_S3-34dk_2026-11-02.md).

#### S3-34dk. Exact harness gates for S3-20fz

**Статус реализации и независимого статического review: PASS; исполнение harness отложено.** Harness commit `2de1cab` закрепляет отдельные JIT source/site и host behavior identities для exact addr32 RCX-base Dword load `67 8B 81 id`. Self-test fail-closed gate проверяет отсутствие identity, ошибочные Rust identity/argv, нулевое совпадение и отсутствующее host evidence. Manifest totals 300 gates (290 acceptance + 10 lifecycle), 318 steps и 134 self-test names закреплены и статически reviewed, но не исполнены. Rust gates, Python harness, сборка, форматирование, Cargo и Mini не запускались. См. [S3-34dk report](../reports/STAGE_3_S3-34dk_2026-11-02.md) и [S3-20fz report](../reports/STAGE_3_S3-20fz_2026-11-02.md).

#### S3-20ga. Address32 RCX-base Dword store signed disp32 `67 89 81 id`

**Статус реализации и независимого статического Sol/high review: PASS; поведенческие проверки отложены.** Runtime commit `b7b361b` интегрирован.

Поддержать ровно семь байт `67 89 81 id0 id1 id2 id3`: `MOV [ECX+signext(disp32)],EAX`; `67` — единственный prefix, data/address sizes — Dword, база — addr32 alias RCX, без index, scale 1, disp32 — signed little-endian i32. Стартовый EA вычисляется из pre-state: `zero_extend((low32(RCX) + signed_i32_disp32) mod 2^32)`. Выполняется ровно одна contiguous checked write шириной четыре байта с low32 исходного RAX. Все GPR, включая полный RAX и RCX, и RFLAGS сохраняются; успешный RIP — `A+7`. По модулю 2^32 вычисляется только начальный EA; transfer после `0xffffffff` продолжается contiguous выше 4 GiB без побайтного wrap.

Срез исключает addr32 RCX disp32 load (`67 8B 81 id`), addr32 RCX singleton/disp8 формы (`67 89 01`, `67 89 41 ib`), все другие bases, SIB/index, дополнительные prefixes/REX и прочие encoding формы. Каждая уже поддержанная соседняя форма должна сохранять собственную source/site binding; не превращать её в negative ради нового допуска. Отклонять truncated и trailing-byte варианты.

Существующие memory/backend fault semantics сохраняются: ошибка записи не коммитит CPU state/RIP; успешный fault-free store сохраняет полный исходный CPU state и коммитит ровно один instruction. Entry STOP/stop-RIP/budget guards не делают store; retry после guard или fault использует прежнее исходное состояние/EA и не дублирует write. Callback STOP после завершённой записи не откатывает committed store и не запускает его повторно.

Published IR runtime использует immutable compiler-issued source bytes и не fetch-ит guest instruction во время исполнения. Ошибки fetch байтов инструкции проверяются по demand interpreter path и распространяются напрямую.

Фактические Rust identities: `memory_load_v2::tests::ir_dword_addr32_rcx_disp32_store_binds_exact_source_and_site` и `tests::s3_20ga_addr32_rcx_disp32_dword_store_uses_wrapped_ecx_and_low_eax_payload`. Harness gates: `s3_34dl_ir_addr32_rcx_disp32_dword_store_source_binding`, `s3_34dl_host_addr32_rcx_disp32_dword_store_wrapped_ecx_and_low_eax_payload`; fail-closed self-test: `test_s3_34dl_addr32_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed`. Runtime `b7b361b` and harness `c2a3e8f` интегрированы и прошли независимое статическое Sol/high review.

План приёмки должен связать все семь source bytes, signed metadata и конкретный source/site через compiler, decoded, IR, runtime и host Store publisher; проверить PC-map boundary `A+7`, forged metadata, отсутствие alias с `67 89 01`, `67 89 41 ib`, `67 8B 81 id` и unsupported neighbors. Host coverage сравнивает полное CPU/RAM state и callback address/width/count: signed disp32 edges, high-RCX/low32 decoys, modulo-32 wrap в обе стороны, unaligned/zero/boundary starts `0xfffffffd…0xffffffff`, contiguous write через 4 GiB, отделение low32 payload от address, memory/backend fault и retry, каждую entry guard, committed callback STOP без replay.

Totals after S3-34dl are pinned and statically reviewed: 302 gates (292 acceptance + 10 lifecycle), 320 local steps, 135 self-test names. No gates were executed; behavior tests, build, formatting, Cargo, Python harness and Mini were not run and remain deferred. Independent runtime and harness review passed. Stage 2 and production owner/MMU are outside scope; Stage 3 remains partial. See [S3-20ga report](../reports/STAGE_3_S3-20ga_2026-11-02.md) and [S3-34dl report](../reports/STAGE_3_S3-34dl_2026-11-02.md).

#### S3-34dl. Exact harness gates for S3-20ga

**Статус реализации и независимого статического Sol/high review: PASS; исполнение harness отложено.** Harness commit `c2a3e8f` закрепляет exact JIT source/site и host store identities для `67 89 81 id`, а также fail-closed self-test на Rust identity/argv, нулевой match и итоговое host evidence. Totals 302 gates (292 acceptance + 10 lifecycle), 320 steps и 135 self-test names pinned/reviewed, но не исполнены. Rust gates, Python harness, build, formatting, Cargo и Mini для этого среза не запускались. См. [S3-34dl report](../reports/STAGE_3_S3-34dl_2026-11-02.md) и [S3-20ga report](../reports/STAGE_3_S3-20ga_2026-11-02.md).

#### S3-34dm. Fixed-seed differential corpus for S3-20eg

**Статус реализации и независимого Sol/high review: PASS; bounded local corpus acceptance: PASS.**

Corpus implementation commit `c7bb9c7` and harness commit `2fb1265` are
integrated. The S3-34.1/.3/.5 implementation and gates passed independent
static Sol/high review. The exact od-host corpus gate passed 1/1 and the
fail-closed Python harness self-test exited 0. This is local macOS/aarch64
acceptance only.

Add one deterministic differential host test for the existing exact S3-20eg
singleton `67 89 04 24` (`MOV [ESP],EAX`). It uses three fixed seeds
(`0x2034_d001`, `0x2034_d002`, `0x2034_d003`) with a local SplitMix64
stream to generate 400 unique initial CPU/RAM inputs per seed (1,200 total).
The test verifies uniqueness within and across seeds. The generator is local;
it does not introduce a persisted corpus, replay envelope, reducer API, or
S3-34.2 work. Keep the source bytes and runtime semantics unchanged.

For every input, run both interpreter and JIT twice from independently reset
copies of the identical CPU and RAM pre-state. Compare complete CPU state,
complete fixture RAM state, `RIP == A+4`, one retired instruction, exactly
one contiguous four-byte store callback at `u64(RSP as u32)`, and unchanged
canary bytes immediately around the target span. The callback must write the
low 32 bits of RAX once, including when its four-byte span crosses 4 GiB.
Require deterministic equality between both reset-state runs as well as
interpreter/JIT parity. Inputs cover zero and unaligned starts, ordinary
starts, and every start from `0xffff_fffd` through `0xffff_ffff`; use varied
upper RSP bits, RAX payloads, and the remaining defined CPU state while
keeping each generated input distinct. The transfer bytes after `0xffff_ffff`
remain contiguous above 4 GiB.

This corpus covers successful execution only. Fault/retry and callback STOP
semantics remain in the existing focused S3-20eg checks; no additional fault
or retry behavior is introduced. Runtime semantic contracts do not change.

Rust identity:
`tests::fixed_seed_addr32_rsp_dword_store_corpus_matches_interpreter_twice_per_input`.
Fail-closed harness gate:
`s3_34dm_addr32_rsp_dword_store_fixed_seed_corpus`.

Exact commands and outcomes:

- `cargo test -p od-host --test stage3_dword_store_corpus fixed_seed_addr32_rsp_dword_store_corpus_matches_interpreter_twice_per_input --offline -- --exact --test-threads=1` — passed 1/1.
- `PYTHONPATH=tests/cpu python3 -m unittest -v test_stage3_harness.Stage3HarnessValidationTests.test_s3_34dm_addr32_rsp_dword_store_corpus_gate_is_pinned_and_fail_closed` — exited 0.

The Rust gate exercised the 1,200 unique initial states twice per backend
against the interpreter. This is bounded local macOS/aarch64 corpus acceptance;
it is not physical-x86 or Mini evidence and does not complete Stage 3. At the
S3-34dm cut the pinned totals were 303 gates (293 acceptance + 10 lifecycle),
321 local steps, and 136 self-test names. The current manifest after S3-34dr
reports 324 gates (306 acceptance + 18 lifecycle), 342 steps, 149 self-test
names, and 18 source pins; this exact run did not execute the full manifest.
Build, formatting, full harness execution, Mini, and physical-x86 acceptance
remain open. Production owner/MMU integration and performance acceptance are
outside this slice; M1 remains deferred.

S3-34.2 runtime/replay and reducer implementation was added in S3-34dn/do,
while broader execution acceptance remains pending. ISSUE-045 was resolved
separately; its closure is recorded in [the issue registry](../KNOWN_ISSUES.md)
and is not attributed to this corpus result. See the
[S3-34dm report](../reports/STAGE_3_S3-34dm_2026-11-02.md) and the
[JIT testing reference](../testing/JIT.md#s3-34dm-fixed-seed-differential-corpus-for-s3-20eg).

#### S3-34dn. Create-only mismatch artifact and validated replay for S3-34dm

**Статус реализации и независимого статического Sol/high review: PASS; исполнение отложено.** Runtime commits `f858b7a`/`10447f0`/`ee2af23` and harness commits `89c5e28`/`13b7790` are integrated. Runtime, self-check, and harness diffs passed independent static review. Commit `ee2af23` adds direct self-check assertions for create-only/no-overwrite behavior and rejection of changed version and RIP. No tests or self-checks were run.

Add one profile-specific diagnostic path for the existing S3-34dm corpus and
its exact singleton `67 89 04 24` (`MOV [ESP],EAX`). It creates a persisted
artifact only when the deterministic differential check finds a mismatch.
Creation must be create-only: an existing artifact is never replaced. The
artifact records the complete initial CPU state and fixture RAM bytes/layout,
instruction address/RIP, exact source bytes and profile identity, the S3-34dm
seed and case index as provenance, both backends' repeated observations, and
the exact mismatch classification/details. It is a single-profile diagnostic
record, not a general corpus or replay format.

The versioned 15-field artifact schema validates profile and source bytes,
fixed RIP, GPR/flags/address/RAM lengths and constraints, seed/index provenance,
fixed sparse RAM layout, and an FNV-1a 64-bit digest of the saved RAM layout
and bytes before compiling or publishing code. Invalid or mismatched fields
are rejected before execution. The saved initial CPU/RAM state is the replay
input; seed and case index are provenance only and do not regenerate or
replace that state. Replay runs interpreter and JIT independently twice from
reset copies of the saved state. Report `reproduced` only when both repeats are
deterministic and the recorded mismatch identity and captured observations
recur. Report `not_reproduced` when they do not. This is diagnostic evidence,
not a passing regression gate. The optional replay test also emits
`not_reproduced` and returns when no artifact path is supplied or its path is
absent; that outcome is not evidence that replay occurred. Malformed or
incompatible artifacts fail with a validation error rather than being
classified as a valid replay.

The self-check uses explicitly labeled synthetic mismatch data. It checks
serialization/parser round-trip, both synthetic classifier outcomes,
fail-closed rejection of changed profile/source/address provenance/RAM
layout/digest/version/RIP, and create-only behavior by preserving a pre-existing
sentinel file. It does not execute either backend. Synthetic self-check results
must remain labeled and must not be presented as runtime mismatch or replay
evidence. The expanded self-check passed independent static Sol/high review,
and its exact Rust gate and pinned Python self-test passed. No new runtime API,
dependency, opcode, generic replay framework, or reducer is in scope. The
bounded S3-34.2 artifact/replay subtask was implemented here; the reducer was
subsequently implemented in S3-34do. Full execution acceptance remains
pending.

The Rust self-check identity is
`s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed`; the optional
saved-artifact replay identity is `replay_saved_s3_34dn_mismatch_artifact`.
Harness gate: `s3_34dn_replay_artifact_self_check`; its fail-closed self-test
is `test_s3_34dn_replay_artifact_self_check_gate_is_pinned_and_fail_closed`.
At the S3-34dn integration revision, the manifest contained 304 gates (294
acceptance + 10 lifecycle), 322 local steps, and 137 self-test names. The
current manifest after S3-34dr contains 324 gates (306 acceptance + 18
lifecycle), 342 local steps, and 149 self-test names. The Rust artifact
self-check and pinned harness self-test passed, but use synthetic observations;
they do not establish JIT parity or real-artifact replay. No JIT runtime
behavior gate, build, formatting, Mini test, or M1 acceptance is claimed.
ISSUE-045 is resolved for the separate S3-20eg retry-assertion host gate;
ISSUE-046 is resolved for the S3-34dn artifact-protection self-check only. See the
[S3-34dn report](../reports/STAGE_3_S3-34dn_2026-11-03.md) and the
[JIT testing reference](../testing/JIT.md#s3-34dn-create-only-mismatch-artifact-and-validated-replay-for-s3-34dm).

#### S3-34do. Bounded value-only reducer for one S3-34dn artifact

**Статус:** пройден локальный сценарий сохранения, повтора и сокращения
артефакта с контролируемым расхождением. Оба движка исполняют `67 89 04 24`
с исходным набором и с кандидатами reducer. Тест меняет бит R9 в наблюдении
после успешного исполнения JIT. С обычным наблюдением повтор выдаёт
`not_reproduced`, а reducer проверяет исходное состояние и останавливается
до перебора кандидатов. С тем же искажением reducer сохраняет уменьшенный
вход, исходный файл остаётся побайтно цел, а повтор уменьшенного входа
подтверждает расхождение. Общая приёмка S3-34 остаётся открытой.

Reduce one validated S3-34dn mismatch artifact for the exact profile
`67 89 04 24`. First replay the original artifact twice from its saved state
using the existing original-artifact classifier. If the original is
`not_reproduced`, stop and create no reduced artifact. Candidate exploration
starts only after the original mismatch has reproduced twice with stable
observations and the exact recorded mismatch signature.

Candidate edits are value-only and bounded to non-address GPR values and bytes
already present in the mapped fixture RAM. Keep source/profile, RIP, effective
address, RSP, flags, seed/index provenance, and RAM address map/layout fixed.
Preserve the callback protocol: exactly one contiguous width-four write at the
fixed address, with callback bytes matching candidate write data and resulting
guest RAM. Do not remove instructions, alter source bytes, change mapping, or
broaden profile admission.

Evaluate candidates in deterministic order with a hard ceiling of 256
candidate attempts per artifact. Each candidate starts from an independent
copy of the current best state and is replayed twice per backend. Candidate
acceptance requires (a) observations that match between each backend's two
repeats, (b) an exact comparator mismatch string equal to the original
artifact's `mismatch` field and the same structured mismatch signature, and
(c) a strictly lower popcount over mutable GPR/RAM values than the current
accepted input. The first passing candidate in the deterministic sequence
becomes the new best input; regenerate candidates from it and repeat until no
candidate passes or the cap is reached. This deterministic greedy search does
not claim a global minimum. If the original was reproduced but no candidate
passes all conditions, report no reduction and write no reduced artifact.

Critical validation boundary: existing `classify_replay` requires the full
candidate observations to equal the artifact's recorded full observations.
Those observations necessarily change when candidate state changes, so that
function can establish only that the original artifact reproduces. It must not
be used to accept reduced candidates. Candidate checks use a separate,
profile-specific ordinary-replay validator with candidate-specific recorded
observations. It verifies deterministic repeated candidate observations, the
exact stable mismatch signature, and the preserved fields above. Do not use
`classify_replay` for candidates or turn this into a generic reducer/replay
framework.

Write a profile-specific reduced artifact using create-only semantics only
after at least one candidate meets every acceptance condition and the chosen
result has lower mutable-value popcount. Never replace the original or an
existing reduced artifact. The reducer has a synthetic self-check and a
separate diagnostic that executes the local interpreter and JIT with a
deliberately altered JIT observation.

The Rust reducer identity is `reduce_saved_s3_34do_value_only_artifact`; its
self-check identity is
`s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature`.
The local harness requires `s3_34do_value_only_reducer_self_check` and
`s3_34do_captured_runtime_mismatch`. The fail-closed harness self-test
`test_s3_34do_value_only_reducer_self_check_gate_is_pinned_and_fail_closed`
checks both exact identities and rejects missing, zero-match, ignored or
failed results. См. [отчёт о проверке](../reports/STAGE_3_S3-34do_2026-11-03.md#controlled-runtime-capture-replay-and-reduction).
The controlled run covers the exact store profile. General S3-34.2 runtime
acceptance remains open. Physical x86 and Mac mini checks are deferred. See
the [JIT testing reference](../testing/JIT.md#s3-34do-bounded-value-only-reducer-for-one-s3-34dn-artifact).

#### S3-34dp. Exact local harness gates for S3-38e session ownership

The harness pins four exact `od-host --lib` lifecycle tests in
`session::tests`: repeated same-code publication reuse with fresh CPU state,
budget and STOP; clear/drop mapping cleanup; rejection of a second live
executable-memory owner; and explicit invalid-entry error reporting without an
automatic retry. The fail-closed self-check verifies unique gate/manifest/step
identities, exact Cargo argv, the `session.rs` source pin, synthetic summary
membership, and rejection of missing, ignored, failed, zero-match, wrong
identity, or wrong-argv evidence.

At integration the harness had 312 Rust gates (298 acceptance + 14 lifecycle),
330 local steps, and 140 self-test names. Current totals after S3-34dr are 324
gates (306 acceptance + 18 lifecycle), 342 steps, 149 self-tests, and 18
sources. Independent static review passed; all four exact lifecycle gates and
the pinned self-check passed locally. The full Stage 3 harness remains
unexecuted. Mini/physical-x86 checks are pending endpoint availability; M1 is
deferred.
The session owner itself is documented in the
[S3-38e report](../reports/STAGE_3_S3-38e_session_owner_2026-11-03.md); see the
[focused harness report](../reports/STAGE_3_S3-34dp_2026-11-03.md) and
[JIT testing reference](../testing/JIT.md#s3-38e-bounded-local-rust-session-owner).

#### S3-34dq. Exact Store→Store corpus, replay, and reducer coverage

`od-testkit` now generates 1,200 fixed-seed inputs for the exact two-site
sequence `48 89 03 48 89 02` (`MOV [RBX],RAX; MOV [RDX],RAX`). The host corpus
compares native and interpreter executions, repeats each input, and covers
second-write architectural faults and continuation while preserving the first
committed write without replay. Saved cases use the distinct profile
`s3-26f-two-site-qword-store-store-v1`; the envelope remains version 1, and
existing memory profiles retain their validation and seeds. The three benchmark
consumers explicitly exclude the new sequence; their profile lists, inputs,
counts, defaults, and cached support remain unchanged.

The harness pins the exact host corpus and testkit generator gates plus the
existing S3-26f JIT/dispatcher gates. Its fail-closed self-check verifies test
identities, source pins, commands, and missing/duplicate evidence. The host
lifecycle regression, three exact `od-host --test memory_corpus` gates, the
exact `od-testkit` generator gate, and S3-26f fail-closed Python self-test all
passed 1/1. Current global manifest totals are 324 Rust gates (306 acceptance
and 18 lifecycle), 342 local steps, 149 self-test names, and 18 source pins; this
slice adds two corpus/generator gates. The full harness unittest timed out
after 180 seconds and was killed without a final result; this is not
classified as a test failure. `graphify update .` after the fixture change
timed out after 120 seconds; refresh is unconfirmed. Mini SSH to
`senya@192.168.1.14:22` timed out before any remote command or test ran. Mini
and physical-x86 acceptance remain pending; M1 remains deferred. Production
owner/MMU and performance acceptance remain open, and ISSUE-045 stays open.
See the [S3-34dq report](../reports/STAGE_3_S3-34dq_2026-11-03.md) and
[JIT testing reference](../testing/JIT.md#s3-34dq-storestore-corpus-replay-and-reducer-coverage).

#### S3-34dr. Exact local harness gates for the S3-18f Load→Load profile

The harness pins the JIT exact-profile admission gate,
`memory_cache::tests::profiles_admit_only_the_exact_load_store_sequences`,
the internal second-site entry rejection gate,
`memory_cache::tests::internal_second_site_rip_remains_rejected_for_load_pair`,
and the retained host regression,
`tests::retained_load_load_pair_uses_fresh_reads_and_continues_after_second_read_fault`.
Each is a distinct exact `od-jit --lib` or `od-host --lib` local-acceptance
identity. The source manifest now includes
`crates/od-jit/src/memory_cache.rs`; the global source fingerprint already
covers tracked files. The fail-closed self-test verifies exact identities,
step/manifest uniqueness, exact Cargo arguments, required source pins, and
rejection of missing, zero-match, or forged-argv evidence.

Totals are 324 Rust gates (306 acceptance + 18 lifecycle), 342 local steps,
149 self-test names, and 18 local Rust source pins. Independent Sol/high review
passed. The new fail-closed self-test and each of the three exact Rust gates
passed 1/1. The full `PYTHONPATH=tests/cpu python3 -m unittest
test_stage3_harness` run exceeded 180 seconds and was killed without a final
result; this is not classified as a test failure. Mini has not run; M1 remains
deferred. See the
[S3-34dr report](../reports/STAGE_3_S3-34dr_2026-11-03.md) and
[JIT testing reference](../testing/JIT.md#s3-34dr-exact-harness-gates-for-the-s3-18f-loadload-profile).
